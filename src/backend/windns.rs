//! The Windows DNS Client service through the DNS-SD API in `dnsapi.dll`:
//! service type browse → service browses → resolves.
//!
//! The API is looked up at run time, so the program still starts on
//! Windows builds without it. Its callbacks run on thread-pool threads and
//! only queue events; one task owns the [`Tracker`], which turns them into
//! observations and into operations to start or stop.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ffi::c_void;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::{Duration, SystemTime};

use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{debug, warn};
use windows_sys::Win32::Foundation::{DNS_REQUEST_PENDING, ERROR_CANCELLED, ERROR_SUCCESS};
use windows_sys::Win32::NetworkManagement::Dns::{
    DNS_FREE_TYPE, DNS_QUERY_REQUEST_VERSION1, DNS_RECORDW, DNS_SERVICE_BROWSE_REQUEST,
    DNS_SERVICE_CANCEL, DNS_SERVICE_INSTANCE, DNS_SERVICE_RESOLVE_REQUEST, DNS_TYPE_A,
    DNS_TYPE_AAAA, DNS_TYPE_PTR, DNS_TYPE_SRV, DNS_TYPE_TEXT, DnsFreeRecordList,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

use super::names::{parse_presentation, unescape_label};
use super::{BoxFuture, SystemBackend};
use crate::model::record::{DnsName, Observation, ObservedRecord, RData, Source, SourceState};
use crate::wire::sys::{list_interfaces, wide};

const TYPES_QUERY: &str = "_services._dns-sd._udp.local";
/// How long a finished operation's context is kept before it is freed, in
/// case a callback for it is still running on another thread.
const RETIRE_AFTER: Duration = Duration::from_secs(5);

type BrowseFn = unsafe extern "system" fn(*const DNS_SERVICE_BROWSE_REQUEST, *mut DNS_SERVICE_CANCEL) -> i32;
type ResolveFn = unsafe extern "system" fn(*const DNS_SERVICE_RESOLVE_REQUEST, *mut DNS_SERVICE_CANCEL) -> i32;
type CancelFn = unsafe extern "system" fn(*const DNS_SERVICE_CANCEL) -> i32;
type FreeInstanceFn = unsafe extern "system" fn(*const DNS_SERVICE_INSTANCE);
type FreeFn = unsafe extern "system" fn(*const c_void, DNS_FREE_TYPE);

/// The `dnsapi.dll` functions the backend needs.
#[derive(Clone, Copy)]
#[allow(non_snake_case)]
struct Api {
    DnsServiceBrowse: BrowseFn,
    DnsServiceBrowseCancel: CancelFn,
    DnsServiceResolve: ResolveFn,
    DnsServiceResolveCancel: CancelFn,
    DnsServiceFreeInstance: FreeInstanceFn,
    DnsFree: FreeFn,
}

/// Why the API could not be loaded.
const API_MISSING: &str = "Windows DNS-SD API not available (needs Windows 10 1809 or later)";

impl Api {
    fn load() -> Result<Self, String> {
        let dll = to_wide("dnsapi.dll");
        // SAFETY: a NUL-terminated name; loading from System32 only.
        let module = unsafe { LoadLibraryExW(dll.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        if module.is_null() {
            return Err(API_MISSING.into());
        }
        Self::load_with(|name| {
            let name = format!("{name}\0");
            // SAFETY: a valid module and a NUL-terminated ASCII name.
            unsafe { GetProcAddress(module, name.as_ptr()) }.map(|f| f as *const c_void)
        })
    }

    /// Builds the table from `get`, which looks up one export by name.
    fn load_with(get: impl Fn(&str) -> Option<*const c_void>) -> Result<Self, String> {
        macro_rules! get {
            ($name:ident: $ty:ty) => {{
                let f = get(stringify!($name)).ok_or_else(|| API_MISSING.to_string())?;
                // SAFETY: the export has this signature (from `windns.h`, as
                // in `windows-sys`).
                unsafe { std::mem::transmute::<*const c_void, $ty>(f) }
            }};
        }
        Ok(Self {
            DnsServiceBrowse: get!(DnsServiceBrowse: BrowseFn),
            DnsServiceBrowseCancel: get!(DnsServiceBrowseCancel: CancelFn),
            DnsServiceResolve: get!(DnsServiceResolve: ResolveFn),
            DnsServiceResolveCancel: get!(DnsServiceResolveCancel: CancelFn),
            DnsServiceFreeInstance: get!(DnsServiceFreeInstance: FreeInstanceFn),
            DnsFree: get!(DnsFree: FreeFn),
        })
    }
}

pub struct WinDnsBackend;

impl SystemBackend for WinDnsBackend {
    fn name(&self) -> &'static str {
        "Windows DNS"
    }

    fn run(self: Box<Self>, observations: mpsc::Sender<Observation>) -> BoxFuture {
        Box::pin(run_with(Api::load, observations))
    }
}

async fn run_with(load: fn() -> Result<Api, String>, observations: mpsc::Sender<Observation>) {
    let state = match load() {
        Ok(api) => Driver::new(api).run(&observations).await,
        Err(reason) => Some(SourceState::Unavailable(reason)),
    };
    if let Some(state) = state {
        if let SourceState::Unavailable(reason) = &state {
            warn!("Windows DNS unavailable: {reason}");
        }
        let _ = observations
            .send(Observation::SourceStatus {
                source: Source::System,
                state,
            })
            .await;
    }
}

/// Identifies one API operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum OpKey {
    /// The `_services._dns-sd._udp` browse for service types.
    Types,
    /// Browse for instances of a service type (name key, e.g. `_ipp._tcp.local`).
    Browse(String),
    /// Resolve an instance (name key).
    Resolve(String),
}

/// One record from a browse callback, copied out of the API's list.
#[derive(Debug, Clone, PartialEq)]
struct BrowseRecord {
    name: String,
    /// Not a TTL: the DNS Client reports when the record expires, in
    /// seconds of an internal clock, i.e. clock + TTL (see [`Tracker::goodbye`]).
    expiry: u32,
    data: RecordData,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RecordData {
    Ptr(String),
    Srv {
        priority: u16,
        weight: u16,
        port: u16,
        target: String,
    },
    Txt(Vec<String>),
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
}

/// A resolve result, copied out of the API's `DNS_SERVICE_INSTANCE`.
#[derive(Debug, Clone, PartialEq)]
struct Resolved {
    ifindex: u32,
    host: String,
    port: u16,
    priority: u16,
    weight: u16,
    /// TXT as key/value properties; the value is `None` when the API gave none.
    properties: Vec<(String, Option<String>)>,
    v4: Option<Ipv4Addr>,
    v6: Option<Ipv6Addr>,
}

/// What a callback reported, for the tracker.
#[derive(Debug, Clone, PartialEq)]
enum Event {
    Browse { key: OpKey, records: Vec<BrowseRecord> },
    /// A browse stopped on its own with an error.
    BrowseEnded { key: OpKey, status: u32 },
    /// A resolve completed; `None` when it failed.
    Resolved { key: OpKey, result: Option<Resolved> },
}

/// An operation for the driver to start or stop.
#[derive(Debug, Clone, PartialEq)]
enum Command {
    Browse { key: OpKey, query: String },
    Resolve { key: OpKey, query: String },
    Stop(OpKey),
}

#[derive(Debug, Default)]
struct InstanceState {
    iface: Option<String>,
    /// Whether a browse result carried TXT for it; then the resolve's
    /// rebuilt key/value TXT is not used.
    browse_txt: bool,
    resolving: bool,
}

/// API-side bookkeeping, independent of the FFI so it can be tested.
struct Tracker {
    /// Type name key → type name, for every running type browse.
    types: HashMap<String, DnsName>,
    instances: HashMap<String, (DnsName, InstanceState)>,
    /// Host name key → interface name, learned from resolves.
    hosts: HashMap<String, String>,
    /// Last reported expiry of each record, to spot goodbyes.
    expiries: HashMap<(String, RecordData), u32>,
    names: HashMap<u32, Option<String>>,
    interface_name: Box<dyn Fn(u32) -> Option<String> + Send>,
}

impl Tracker {
    fn new(interface_name: Box<dyn Fn(u32) -> Option<String> + Send>) -> Self {
        Self {
            types: HashMap::new(),
            instances: HashMap::new(),
            hosts: HashMap::new(),
            expiries: HashMap::new(),
            names: HashMap::new(),
            interface_name,
        }
    }

    fn iface(&mut self, index: u32) -> Option<String> {
        let lookup = &self.interface_name;
        self.names.entry(index).or_insert_with(|| lookup(index)).clone()
    }

    /// Whether a record is a goodbye. The API reports a goodbye as the
    /// record expiring now, so its expiry drops below the one reported
    /// before; while that clock is near zero it can also be 0 itself.
    fn goodbye(&mut self, r: &BrowseRecord) -> bool {
        let key = (r.name.to_ascii_lowercase(), r.data.clone());
        let previous = self.expiries.insert(key.clone(), r.expiry);
        let gone = r.expiry == 0 || previous.is_some_and(|p| r.expiry.saturating_add(1) < p);
        if gone {
            self.expiries.remove(&key);
        }
        gone
    }

    fn instance_iface(&self, instance: &DnsName) -> Option<String> {
        self.instances.get(instance.key()).and_then(|(_, s)| s.iface.clone())
    }

    fn handle(&mut self, event: Event, out: &mut Vec<Observation>, commands: &mut Vec<Command>) {
        match event {
            Event::Browse { key: OpKey::Types, records } => {
                for r in records {
                    let RecordData::Ptr(target) = &r.data else { continue };
                    let service_type = parse_presentation(target);
                    if self.goodbye(&r) || !service_type.is_service_type() {
                        continue;
                    }
                    if let Entry::Vacant(e) = self.types.entry(service_type.key().to_string()) {
                        e.insert(service_type.clone());
                        commands.push(Command::Browse {
                            key: OpKey::Browse(service_type.key().to_string()),
                            query: service_type.to_string(),
                        });
                    }
                    out.push(observe(parse_presentation(&r.name), RData::Ptr(service_type), None));
                }
            }
            Event::Browse {
                key: OpKey::Browse(type_key),
                records,
            } => {
                let Some(service_type) = self.types.get(&type_key).cloned() else {
                    return; // stopped since the reply was queued
                };
                for r in records {
                    self.browse_record(r, &service_type, out, commands);
                }
            }
            Event::Browse { .. } => {}
            Event::BrowseEnded { key, status } => {
                debug!("Windows DNS browse {key:?} ended with status {status}");
                match key {
                    // Forgotten, so the next report of the type starts it again.
                    OpKey::Browse(t) => {
                        self.types.remove(&t);
                    }
                    OpKey::Types => warn!("Windows DNS service type browse ended with status {status}"),
                    OpKey::Resolve(_) => {}
                }
            }
            Event::Resolved {
                key: OpKey::Resolve(instance_key),
                result,
            } => {
                let iface = result.as_ref().and_then(|r| self.iface(r.ifindex));
                let Some((instance, state)) = self.instances.get_mut(&instance_key) else {
                    return; // removed since the reply was queued
                };
                state.resolving = false;
                let Some(r) = result else {
                    return;
                };
                if iface.is_some() {
                    state.iface = iface.clone();
                }
                let instance = instance.clone();
                let use_txt = !state.browse_txt && !r.properties.is_empty();
                let host = parse_presentation(&r.host);
                if let Some(name) = &iface {
                    self.hosts.insert(host.key().to_string(), name.clone());
                }
                out.push(observe(
                    instance.clone(),
                    RData::Srv {
                        priority: r.priority,
                        weight: r.weight,
                        port: r.port,
                        target: host.clone(),
                    },
                    iface.clone(),
                ));
                if use_txt {
                    out.push(observe(instance, RData::Txt(txt_from_properties(&r.properties)), iface.clone()));
                }
                if let Some(a) = r.v4 {
                    out.push(observe(host.clone(), RData::A(a), iface.clone()));
                }
                if let Some(a) = r.v6 {
                    out.push(observe(host, RData::Aaaa(a), iface));
                }
            }
            Event::Resolved { .. } => {}
        }
    }

    fn browse_record(
        &mut self,
        r: BrowseRecord,
        service_type: &DnsName,
        out: &mut Vec<Observation>,
        commands: &mut Vec<Command>,
    ) {
        let goodbye = self.goodbye(&r);
        match r.data {
            RecordData::Ptr(target) => {
                let instance = instance_in(&target, service_type);
                let key = instance.key().to_string();
                if goodbye {
                    if let Some((_, state)) = self.instances.remove(&key) {
                        if state.resolving {
                            commands.push(Command::Stop(OpKey::Resolve(key)));
                        }
                        out.push(Observation::SystemRemoved {
                            instance,
                            at: Instant::now(),
                        });
                    }
                    return;
                }
                if let Entry::Vacant(e) = self.instances.entry(key.clone()) {
                    e.insert((
                        instance.clone(),
                        InstanceState {
                            resolving: true,
                            ..Default::default()
                        },
                    ));
                    commands.push(Command::Resolve {
                        key: OpKey::Resolve(key),
                        query: target,
                    });
                }
                let iface = self.instance_iface(&instance);
                out.push(observe(service_type.clone(), RData::Ptr(instance), iface));
            }
            _ if goodbye => {} // goodbyes count only for the PTR
            RecordData::Srv {
                priority,
                weight,
                port,
                target,
            } => {
                let instance = instance_in(&r.name, service_type);
                let iface = self.instance_iface(&instance);
                let target = parse_presentation(&target);
                out.push(observe(
                    instance,
                    RData::Srv {
                        priority,
                        weight,
                        port,
                        target,
                    },
                    iface,
                ));
            }
            RecordData::Txt(strings) => {
                let instance = instance_in(&r.name, service_type);
                if let Some((_, state)) = self.instances.get_mut(instance.key()) {
                    state.browse_txt = true;
                }
                let iface = self.instance_iface(&instance);
                let data = strings.into_iter().map(String::into_bytes).collect();
                out.push(observe(instance, RData::Txt(data), iface));
            }
            RecordData::A(a) => self.host_record(&r.name, RData::A(a), out),
            RecordData::Aaaa(a) => self.host_record(&r.name, RData::Aaaa(a), out),
        }
    }

    fn host_record(&self, name: &str, data: RData, out: &mut Vec<Observation>) {
        let host = parse_presentation(name);
        let iface = self.hosts.get(host.key()).cloned();
        out.push(observe(host, data, iface));
    }
}

/// The instance name in `text` (`Mr. Printer._ipp._tcp.local`), taking
/// everything before the known type as one label: the API does not escape
/// dots inside instance labels.
fn instance_in(text: &str, service_type: &DnsName) -> DnsName {
    let text = text.strip_suffix('.').unwrap_or(text);
    let suffix = format!(".{service_type}");
    if let Some(split) = text.len().checked_sub(suffix.len()).filter(|&i| i > 0)
        && text.is_char_boundary(split)
        && text[split..].eq_ignore_ascii_case(&suffix)
    {
        let label = unescape_label(&text[..split]);
        return DnsName::from_labels(std::iter::once(label).chain(service_type.labels().iter().cloned()));
    }
    parse_presentation(text)
}

/// TXT strings from the API's key/value form: `key=value`, or `key` alone
/// when there is no value.
fn txt_from_properties(properties: &[(String, Option<String>)]) -> Vec<Vec<u8>> {
    properties
        .iter()
        .map(|(k, v)| match v.as_deref() {
            None | Some("") => k.clone().into_bytes(),
            Some(v) => format!("{k}={v}").into_bytes(),
        })
        .collect()
}

fn observe(name: DnsName, data: RData, iface: Option<String>) -> Observation {
    Observation::Record(ObservedRecord {
        name,
        data,
        ttl: None,
        cache_flush: false,
        source: Source::System,
        src_addr: None,
        iface,
        at: Instant::now(),
        wall: SystemTime::now(),
    })
}

/// The interface name for an index, the same way the wire source names it.
fn interface_name(index: u32) -> Option<String> {
    list_interfaces()
        .ok()?
        .into_iter()
        .find(|i| i.index == index)
        .map(|i| i.name)
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---- FFI driver ------------------------------------------------------------

/// What a callback needs: where to queue events and which operation it is.
struct OpContext {
    tx: mpsc::UnboundedSender<Raw>,
    id: u64,
    api: Api,
}

/// A callback report, before the driver maps it to its operation.
struct Raw {
    id: u64,
    kind: RawKind,
}

enum RawKind {
    Records(Vec<BrowseRecord>),
    /// The browse's last callback: cancelled or failed.
    BrowseDone(u32),
    /// The resolve's only callback.
    ResolveDone(Option<Resolved>),
}

/// A running operation. Its context is freed only after its last callback
/// (plus [`RETIRE_AFTER`]), never while the API may still call it.
struct Op {
    key: OpKey,
    resolve: bool,
    cancel: CancelHandle,
    context: Box<OpContext>,
    stopping: bool,
}

/// The API's cancel token for one operation.
struct CancelHandle(Box<DNS_SERVICE_CANCEL>);

// SAFETY: the token is opaque data that the cancel functions accept from
// any thread; it is only read by them.
unsafe impl Send for CancelHandle {}

struct Driver {
    api: Api,
    tracker: Tracker,
    tx: mpsc::UnboundedSender<Raw>,
    rx: mpsc::UnboundedReceiver<Raw>,
    next_id: u64,
    ops: HashMap<u64, Op>,
    by_key: HashMap<OpKey, u64>,
    retired: Vec<(Instant, Box<OpContext>)>,
}

impl Driver {
    fn new(api: Api) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            api,
            tracker: Tracker::new(Box::new(interface_name)),
            tx,
            rx,
            next_id: 0,
            ops: HashMap::new(),
            by_key: HashMap::new(),
            retired: Vec::new(),
        }
    }

    /// Runs until the observation channel closes. Returns the state to
    /// report when the API cannot be used.
    async fn run(mut self, observations: &mpsc::Sender<Observation>) -> Option<SourceState> {
        if let Err(code) = self.start(OpKey::Types, TYPES_QUERY.into(), false) {
            return Some(SourceState::Unavailable(format!(
                "DnsServiceBrowse failed: {}",
                io::Error::from_raw_os_error(code)
            )));
        }
        if observations
            .send(Observation::SourceStatus {
                source: Source::System,
                state: SourceState::Active,
            })
            .await
            .is_err()
        {
            self.shutdown();
            return None;
        }
        let mut sweep = tokio::time::interval(RETIRE_AFTER);
        loop {
            let raw = tokio::select! {
                raw = self.rx.recv() => raw,
                _ = observations.closed() => break,
                _ = sweep.tick() => {
                    let now = Instant::now();
                    self.retired.retain(|(at, _)| now.duration_since(*at) < RETIRE_AFTER);
                    continue;
                }
            };
            // `self.tx` keeps the channel open, so `recv` never returns None.
            let Some(raw) = raw else { break };
            let Some(event) = self.event(raw) else { continue };
            let mut out = Vec::new();
            let mut commands = Vec::new();
            self.tracker.handle(event, &mut out, &mut commands);
            for command in commands {
                self.execute(command);
            }
            for obs in out {
                if observations.send(obs).await.is_err() {
                    self.shutdown();
                    return None;
                }
            }
        }
        self.shutdown();
        None
    }

    /// Maps a callback report to a tracker event, retiring finished operations.
    fn event(&mut self, raw: Raw) -> Option<Event> {
        let op = self.ops.get(&raw.id)?;
        let last = !matches!(raw.kind, RawKind::Records(_));
        let (key, stopping) = (op.key.clone(), op.stopping);
        if last {
            if self.by_key.get(&key) == Some(&raw.id) {
                self.by_key.remove(&key);
            }
            let op = self.ops.remove(&raw.id)?;
            self.retired.push((Instant::now(), op.context));
        }
        if stopping {
            return None;
        }
        Some(match raw.kind {
            RawKind::Records(records) => Event::Browse { key, records },
            RawKind::BrowseDone(status) => Event::BrowseEnded { key, status },
            RawKind::ResolveDone(result) => Event::Resolved { key, result },
        })
    }

    fn execute(&mut self, command: Command) {
        match command {
            Command::Browse { key, query } => {
                if let Err(code) = self.start(key.clone(), query, false) {
                    debug!("cannot browse {key:?}: error {code}");
                    self.tracker.handle(Event::BrowseEnded { key, status: code as u32 }, &mut Vec::new(), &mut Vec::new());
                }
            }
            Command::Resolve { key, query } => {
                if let Err(code) = self.start(key.clone(), query, true) {
                    debug!("cannot resolve {key:?}: error {code}");
                    self.tracker.handle(Event::Resolved { key, result: None }, &mut Vec::new(), &mut Vec::new());
                }
            }
            Command::Stop(key) => self.stop(&key),
        }
    }

    /// Starts a browse or resolve. On error no callback will come.
    fn start(&mut self, key: OpKey, query: String, resolve: bool) -> Result<(), i32> {
        let id = self.next_id;
        self.next_id += 1;
        let context = Box::new(OpContext {
            tx: self.tx.clone(),
            id,
            api: self.api,
        });
        let mut cancel = Box::new(DNS_SERVICE_CANCEL::default());
        let mut name = to_wide(&query);
        let context_ptr = (&*context as *const OpContext).cast_mut().cast();
        // SAFETY: the request points to live buffers for the duration of the
        // call; the context stays alive until after the last callback.
        let rc = unsafe {
            if resolve {
                let request = DNS_SERVICE_RESOLVE_REQUEST {
                    Version: DNS_QUERY_REQUEST_VERSION1,
                    InterfaceIndex: 0,
                    QueryName: name.as_mut_ptr(),
                    pResolveCompletionCallback: Some(on_resolve),
                    pQueryContext: context_ptr,
                };
                (self.api.DnsServiceResolve)(&request, &mut *cancel)
            } else {
                let mut request = DNS_SERVICE_BROWSE_REQUEST {
                    Version: DNS_QUERY_REQUEST_VERSION1,
                    InterfaceIndex: 0,
                    QueryName: name.as_ptr(),
                    pQueryContext: context_ptr,
                    ..Default::default()
                };
                request.Anonymous.pBrowseCallback = Some(on_browse);
                (self.api.DnsServiceBrowse)(&request, &mut *cancel)
            }
        };
        if rc != DNS_REQUEST_PENDING {
            return Err(rc);
        }
        self.by_key.insert(key.clone(), id);
        self.ops.insert(
            id,
            Op {
                key,
                resolve,
                cancel: CancelHandle(cancel),
                context,
                stopping: false,
            },
        );
        Ok(())
    }

    /// Cancels an operation. It stays in `ops` until its last callback.
    fn stop(&mut self, key: &OpKey) {
        let Some(id) = self.by_key.remove(key) else { return };
        let Some(op) = self.ops.get_mut(&id) else { return };
        op.stopping = true;
        // SAFETY: the handle filled in by the start call of a running operation.
        let rc = unsafe {
            if op.resolve {
                (self.api.DnsServiceResolveCancel)(&*op.cancel.0)
            } else {
                (self.api.DnsServiceBrowseCancel)(&*op.cancel.0)
            }
        };
        if rc != ERROR_SUCCESS as i32 {
            debug!("cancelling {key:?} failed with error {rc}");
        }
    }

    /// Cancels everything. Contexts of operations that may still call back
    /// are leaked: the process is ending or the backend is gone for good.
    fn shutdown(&mut self) {
        let keys: Vec<OpKey> = self.by_key.keys().cloned().collect();
        for key in keys {
            self.stop(&key);
        }
        for (_, op) in self.ops.drain() {
            Box::leak(op.context);
        }
        for (_, context) in self.retired.drain(..) {
            Box::leak(context);
        }
    }
}

/// Clones the sender before sending, so the context is not used after the
/// report that may let the driver retire it.
unsafe fn report(context: *const c_void, kind: RawKind) {
    // SAFETY: `context` is the `OpContext` of a running operation.
    let context = unsafe { &*(context as *const OpContext) };
    let (tx, id) = (context.tx.clone(), context.id);
    let _ = tx.send(Raw { id, kind });
}

unsafe extern "system" fn on_browse(status: u32, context: *const c_void, records: *const DNS_RECORDW) {
    // SAFETY: the API passes our context and, on success, a record list
    // that we own and free.
    unsafe {
        let api = (*(context as *const OpContext)).api;
        if !records.is_null() {
            let copied = copy_records(records);
            (api.DnsFree)(records.cast(), DnsFreeRecordList);
            if status == ERROR_SUCCESS {
                report(context, RawKind::Records(copied));
                return;
            }
        }
        if status != ERROR_SUCCESS {
            if status != ERROR_CANCELLED {
                debug!("Windows DNS browse callback status {status}");
            }
            report(context, RawKind::BrowseDone(status));
        }
    }
}

unsafe extern "system" fn on_resolve(status: u32, context: *const c_void, instance: *const DNS_SERVICE_INSTANCE) {
    // SAFETY: the API passes our context and, on success, an instance that
    // we own and free.
    unsafe {
        let api = (*(context as *const OpContext)).api;
        let result = if instance.is_null() {
            None
        } else {
            let copied = copy_instance(&*instance);
            (api.DnsServiceFreeInstance)(instance);
            (status == ERROR_SUCCESS).then_some(copied)
        };
        report(context, RawKind::ResolveDone(result));
    }
}

/// Copies the records the backend uses out of a `DNS_RECORD` list.
///
/// # Safety
/// `record` must be null or the head of a valid record list.
unsafe fn copy_records(mut record: *const DNS_RECORDW) -> Vec<BrowseRecord> {
    let mut out = Vec::new();
    while !record.is_null() {
        // SAFETY: a node of the list; `wType` selects the union member.
        unsafe {
            let r = &*record;
            let data = match r.wType {
                DNS_TYPE_PTR => Some(RecordData::Ptr(wide(r.Data.PTR.pNameHost))),
                DNS_TYPE_SRV => {
                    let srv = &r.Data.SRV;
                    Some(RecordData::Srv {
                        priority: srv.wPriority,
                        weight: srv.wWeight,
                        port: srv.wPort,
                        target: wide(srv.pNameTarget),
                    })
                }
                DNS_TYPE_TEXT => {
                    let txt = &r.Data.TXT;
                    let strings = std::slice::from_raw_parts(txt.pStringArray.as_ptr(), txt.dwStringCount as usize);
                    Some(RecordData::Txt(strings.iter().map(|&s| wide(s)).collect()))
                }
                DNS_TYPE_A => Some(RecordData::A(Ipv4Addr::from(u32::from_be(r.Data.A.IpAddress)))),
                DNS_TYPE_AAAA => Some(RecordData::Aaaa(Ipv6Addr::from(r.Data.AAAA.Ip6Address.IP6Byte))),
                _ => None,
            };
            if let Some(data) = data {
                out.push(BrowseRecord {
                    name: wide(r.pName),
                    expiry: r.dwTtl,
                    data,
                });
            }
            record = r.pNext;
        }
    }
    out
}

/// # Safety
/// `i` must be a valid instance from a resolve callback.
unsafe fn copy_instance(i: &DNS_SERVICE_INSTANCE) -> Resolved {
    // SAFETY: the arrays have `dwPropertyCount` entries; values may be null.
    unsafe {
        let count = i.dwPropertyCount as usize;
        let properties = if count == 0 || i.keys.is_null() {
            Vec::new()
        } else {
            let keys = std::slice::from_raw_parts(i.keys, count);
            (0..count)
                .map(|n| {
                    let value = (!i.values.is_null())
                        .then(|| *i.values.add(n))
                        .filter(|v| !v.is_null())
                        .map(|v| wide(v));
                    (wide(keys[n]), value)
                })
                .collect()
        };
        Resolved {
            ifindex: i.dwInterfaceIndex,
            host: wide(i.pszHostName),
            port: i.wPort,
            priority: i.wPriority,
            weight: i.wWeight,
            properties,
            v4: (!i.ip4Address.is_null()).then(|| Ipv4Addr::from(u32::from_be(*i.ip4Address))),
            v6: (!i.ip6Address.is_null()).then(|| Ipv6Addr::from((*i.ip6Address).IP6Byte)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> Tracker {
        Tracker::new(Box::new(|i| (i == 14).then(|| "WLAN".to_string())))
    }

    fn handle(t: &mut Tracker, event: Event) -> (Vec<Observation>, Vec<Command>) {
        let mut out = Vec::new();
        let mut commands = Vec::new();
        t.handle(event, &mut out, &mut commands);
        (out, commands)
    }

    fn record(name: &str, expiry: u32, data: RecordData) -> BrowseRecord {
        BrowseRecord {
            name: name.into(),
            expiry,
            data,
        }
    }

    fn types(names: &[&str]) -> Event {
        Event::Browse {
            key: OpKey::Types,
            records: names
                .iter()
                .map(|n| record(TYPES_QUERY, 4500, RecordData::Ptr(n.to_string())))
                .collect(),
        }
    }

    const IPP: &str = "_ipp._tcp.local";
    const PRINTER: &str = "Mr. Printer._ipp._tcp.local";

    fn instances(records: Vec<BrowseRecord>) -> Event {
        Event::Browse {
            key: OpKey::Browse(IPP.into()),
            records,
        }
    }

    fn ptr(expiry: u32) -> BrowseRecord {
        record(IPP, expiry, RecordData::Ptr(PRINTER.into()))
    }

    fn resolved(properties: Vec<(&str, Option<&str>)>) -> Event {
        Event::Resolved {
            key: OpKey::Resolve(instance_in(PRINTER, &DnsName::parse(IPP)).key().to_string()),
            result: Some(Resolved {
                ifindex: 14,
                host: "printer.local".into(),
                port: 631,
                priority: 0,
                weight: 0,
                properties: properties
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.map(String::from)))
                    .collect(),
                v4: Some(Ipv4Addr::new(192, 168, 4, 40)),
                v6: None,
            }),
        }
    }

    fn records(out: &[Observation]) -> Vec<(&DnsName, &RData, Option<&str>)> {
        out.iter()
            .filter_map(|o| match o {
                Observation::Record(r) => Some((&r.name, &r.data, r.iface.as_deref())),
                _ => None,
            })
            .collect()
    }

    fn txts(out: &[Observation]) -> Vec<Vec<Vec<u8>>> {
        records(out)
            .into_iter()
            .filter_map(|(_, d, _)| match d {
                RData::Txt(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    fn browsing_ipp() -> Tracker {
        let mut t = tracker();
        handle(&mut t, types(&[IPP]));
        t
    }

    #[test]
    fn meta_ptr_starts_one_type_browse() {
        let mut t = tracker();
        let (out, commands) = handle(&mut t, types(&[IPP, IPP]));
        assert_eq!(
            commands,
            vec![Command::Browse {
                key: OpKey::Browse(IPP.into()),
                query: IPP.into()
            }]
        );
        let r = records(&out);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0.to_string(), TYPES_QUERY);
        assert_eq!(r[0].1, &RData::Ptr(DnsName::parse(IPP)));
        let (_, again) = handle(&mut t, types(&[IPP]));
        assert!(again.is_empty());
    }

    #[test]
    fn non_type_names_are_ignored() {
        let mut t = tracker();
        let (out, commands) = handle(&mut t, types(&["printer.local"]));
        assert!(out.is_empty() && commands.is_empty());
    }

    #[test]
    fn ptr_starts_one_resolve() {
        let mut t = browsing_ipp();
        let (out, commands) = handle(&mut t, instances(vec![ptr(4500)]));
        let instance = instance_in(PRINTER, &DnsName::parse(IPP));
        assert_eq!(
            commands,
            vec![Command::Resolve {
                key: OpKey::Resolve(instance.key().to_string()),
                query: PRINTER.into()
            }]
        );
        assert_eq!(records(&out), vec![(&DnsName::parse(IPP), &RData::Ptr(instance), None)]);
        let (_, again) = handle(&mut t, instances(vec![ptr(4500)]));
        assert!(again.is_empty());
    }

    #[test]
    fn instance_names_keep_dots() {
        let n = instance_in(PRINTER, &DnsName::parse(IPP));
        assert_eq!(n.labels(), ["Mr. Printer", "_ipp", "_tcp", "local"]);
        assert!(n.is_service_instance());
        assert_eq!(instance_in("Mr. Printer._IPP._tcp.local.", &DnsName::parse(IPP)).labels()[0], "Mr. Printer");
        // Not under the type: parsed as a plain name.
        assert_eq!(instance_in("x.other.local", &DnsName::parse(IPP)).labels(), ["x", "other", "local"]);
    }

    #[test]
    fn goodbye_removes_and_stops_resolver() {
        let mut t = browsing_ipp();
        handle(&mut t, instances(vec![ptr(4500)]));
        let (out, commands) = handle(&mut t, instances(vec![ptr(0)]));
        let instance = instance_in(PRINTER, &DnsName::parse(IPP));
        assert_eq!(commands, vec![Command::Stop(OpKey::Resolve(instance.key().to_string()))]);
        assert!(matches!(&out[..], [Observation::SystemRemoved { instance: i, .. }] if *i == instance));
        // A second goodbye for an instance already gone sends nothing.
        let (out, commands) = handle(&mut t, instances(vec![ptr(0)]));
        assert!(out.is_empty() && commands.is_empty());
    }

    #[test]
    fn expiry_dropping_to_now_is_a_goodbye() {
        let mut t = browsing_ipp();
        handle(&mut t, instances(vec![ptr(9500)]));
        // Re-announced: the expiry moves later, or repeats.
        let (out, _) = handle(&mut t, instances(vec![ptr(9502), ptr(9502)]));
        assert!(out.iter().all(|o| matches!(o, Observation::Record(_))));
        // Goodbye at clock 5002: the expiry drops to "now".
        let (out, commands) = handle(&mut t, instances(vec![ptr(5002)]));
        assert!(matches!(&out[..], [Observation::SystemRemoved { .. }]));
        assert_eq!(commands.len(), 1);
        // Announced again later: a new instance.
        let (out, commands) = handle(&mut t, instances(vec![ptr(9600)]));
        assert!(matches!(&out[..], [Observation::Record(_)]));
        assert_eq!(commands.len(), 1, "resolved again");
    }

    #[test]
    fn goodbye_records_besides_the_ptr_are_skipped() {
        let mut t = browsing_ipp();
        let srv = |expiry| {
            record(
                PRINTER,
                expiry,
                RecordData::Srv {
                    priority: 0,
                    weight: 0,
                    port: 631,
                    target: "printer.local".into(),
                },
            )
        };
        handle(&mut t, instances(vec![ptr(9500), srv(5120)]));
        let (out, _) = handle(&mut t, instances(vec![ptr(5003), srv(5003)]));
        assert!(matches!(&out[..], [Observation::SystemRemoved { .. }]), "{out:?}");
    }

    #[test]
    fn goodbye_after_resolve_does_not_stop_it_again() {
        let mut t = browsing_ipp();
        handle(&mut t, instances(vec![ptr(4500)]));
        handle(&mut t, resolved(vec![]));
        let (out, commands) = handle(&mut t, instances(vec![ptr(0)]));
        assert!(commands.is_empty());
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn browse_records_become_observations() {
        let mut t = browsing_ipp();
        let (out, _) = handle(
            &mut t,
            instances(vec![
                ptr(4500),
                record(
                    PRINTER,
                    120,
                    RecordData::Srv {
                        priority: 0,
                        weight: 0,
                        port: 631,
                        target: "printer.local".into(),
                    },
                ),
                record(PRINTER, 120, RecordData::Txt(vec!["rp=ipp/print".into(), "Color".into()])),
                record("printer.local", 120, RecordData::A(Ipv4Addr::new(192, 168, 4, 40))),
                record("printer.local", 120, RecordData::Aaaa("fe80::1".parse().unwrap())),
                record(PRINTER, 0, RecordData::Txt(vec!["old".into()])),
            ]),
        );
        let r = records(&out);
        assert_eq!(r.len(), 5, "{r:?}");
        let instance = instance_in(PRINTER, &DnsName::parse(IPP));
        assert_eq!(r[1].0, &instance);
        assert!(matches!(r[1].1, RData::Srv { port: 631, target, .. } if target.to_string() == "printer.local"));
        assert_eq!(txts(&out), vec![vec![b"rp=ipp/print".to_vec(), b"Color".to_vec()]]);
        assert_eq!(r[3].1, &RData::A(Ipv4Addr::new(192, 168, 4, 40)));
        assert_eq!(r[4].1, &RData::Aaaa("fe80::1".parse().unwrap()));
    }

    #[test]
    fn browse_txt_wins_over_resolve_txt() {
        let mut t = browsing_ipp();
        handle(
            &mut t,
            instances(vec![ptr(4500), record(PRINTER, 120, RecordData::Txt(vec!["flag".into()]))]),
        );
        let (out, _) = handle(&mut t, resolved(vec![("flag", Some(""))]));
        assert!(txts(&out).is_empty());
    }

    #[test]
    fn resolve_txt_used_without_browse_txt() {
        let mut t = browsing_ipp();
        handle(&mut t, instances(vec![ptr(4500)]));
        let (out, _) = handle(&mut t, resolved(vec![("rp", Some("ipp/print")), ("empty", Some("")), ("bare", None)]));
        assert_eq!(
            txts(&out),
            vec![vec![b"rp=ipp/print".to_vec(), b"empty".to_vec(), b"bare".to_vec()]]
        );
    }

    #[test]
    fn resolve_reports_srv_and_address_with_interface() {
        let mut t = browsing_ipp();
        handle(&mut t, instances(vec![ptr(4500)]));
        let (out, _) = handle(&mut t, resolved(vec![]));
        let r = records(&out);
        assert_eq!(r.len(), 2);
        assert!(matches!(r[0].1, RData::Srv { port: 631, .. }));
        assert_eq!(r[0].2, Some("WLAN"));
        assert_eq!((r[1].0.to_string(), r[1].2), ("printer.local".to_string(), Some("WLAN")));
    }

    #[test]
    fn resolve_interface_applies_to_later_browse_records() {
        let mut t = browsing_ipp();
        handle(&mut t, instances(vec![ptr(4500)]));
        handle(&mut t, resolved(vec![]));
        let (out, _) = handle(
            &mut t,
            instances(vec![
                ptr(4500),
                record("printer.local", 120, RecordData::A(Ipv4Addr::new(192, 168, 4, 41))),
            ]),
        );
        assert!(records(&out).iter().all(|(_, _, iface)| *iface == Some("WLAN")));
    }

    #[test]
    fn failed_resolve_and_ended_browse_are_forgotten() {
        let mut t = browsing_ipp();
        handle(&mut t, instances(vec![ptr(4500)]));
        let key = OpKey::Resolve(instance_in(PRINTER, &DnsName::parse(IPP)).key().to_string());
        let (out, _) = handle(&mut t, Event::Resolved { key, result: None });
        assert!(out.is_empty());
        // A goodbye after a failed resolve stops nothing.
        let (_, commands) = handle(&mut t, instances(vec![ptr(0)]));
        assert!(commands.is_empty());
        handle(&mut t, Event::BrowseEnded { key: OpKey::Browse(IPP.into()), status: 1460 });
        let (_, commands) = handle(&mut t, types(&[IPP]));
        assert_eq!(commands.len(), 1, "the type browse starts again");
    }

    #[test]
    fn txt_properties_are_rebuilt() {
        let props = vec![
            ("k".to_string(), Some("v".to_string())),
            ("empty".to_string(), Some(String::new())),
            ("none".to_string(), None),
        ];
        assert_eq!(
            txt_from_properties(&props),
            vec![b"k=v".to_vec(), b"empty".to_vec(), b"none".to_vec()]
        );
    }

    #[test]
    fn missing_function_makes_api_unavailable() {
        let err = Api::load_with(|name| (name != "DnsServiceResolveCancel").then_some(std::ptr::dangling::<c_void>()));
        assert_eq!(err.err().as_deref(), Some(API_MISSING));
    }

    /// Runs against the local DNS Client service:
    /// `cargo test live_windns -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs the DNS Client service and a LAN"]
    async fn live_windns() {
        let (tx, mut rx) = mpsc::channel(4096);
        let task = tokio::spawn(Box::new(WinDnsBackend).run(tx));
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut seen = std::collections::HashSet::new();
        while let Ok(Some(obs)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            match &obs {
                Observation::Record(r) => {
                    seen.insert(r.data.rtype());
                    println!("{:<8} {} {:?}", r.iface.clone().unwrap_or_default(), r.name, r.data);
                }
                other => println!("{other:?}"),
            }
        }
        drop(rx);
        let _ = task.await;
        use crate::model::record::rtype;
        assert!(seen.contains(&rtype::SRV), "no SRV record");
        assert!(seen.contains(&rtype::TXT), "no TXT record");
        assert!(seen.contains(&rtype::A) || seen.contains(&rtype::AAAA), "no address record");
    }

    /// Announces a made-up service from our own socket, then its goodbye, and
    /// checks that the backend reports both:
    /// `cargo test live_windns_goodbye -- --ignored --nocapture`.
    /// Set MDNSVIEW_LIVE_IFACE to pick the interface (default WLAN).
    #[tokio::test]
    #[ignore = "needs the DNS Client service and a LAN"]
    async fn live_windns_goodbye() {
        use simple_dns::rdata::{self, RData as DnsRData};
        use simple_dns::{CLASS, Name, Packet, PacketFlag, ResourceRecord};

        use crate::wire::sys::MdnsSocket;

        const TYPE: &str = "_mdnsviewtest._tcp.local";
        // A new name each run: the DNS Client keeps names from earlier runs
        // cached with their own TTLs.
        let label = format!("Probe 1.2 {}", std::process::id());
        let announcement = |ttl| {
            let mut p = Packet::new_reply(0);
            p.set_flags(PacketFlag::AUTHORITATIVE_ANSWER);
            let ty = Name::new_unchecked(TYPE);
            let meta = Name::new_unchecked(TYPES_QUERY);
            let labels: Vec<_> = [label.as_str(), "_mdnsviewtest", "_tcp", "local"]
                .into_iter()
                .map(|l| simple_dns::Label::new_unchecked(l.as_bytes()))
                .collect();
            let instance = Name::new_with_labels(&labels);
            let host = Name::new_unchecked("mdnsview-test.local");
            p.answers.push(ResourceRecord::new(meta, CLASS::IN, ttl, DnsRData::PTR(rdata::PTR(ty.clone()))));
            p.answers.push(ResourceRecord::new(ty, CLASS::IN, ttl, DnsRData::PTR(rdata::PTR(instance.clone()))));
            let srv = rdata::SRV { priority: 0, weight: 0, port: 4242, target: host.clone() };
            p.answers.push(ResourceRecord::new(instance, CLASS::IN, ttl, DnsRData::SRV(srv)));
            let a = rdata::A { address: u32::from(Ipv4Addr::new(192, 0, 2, 7)) };
            p.answers.push(ResourceRecord::new(host, CLASS::IN, ttl, DnsRData::A(a)));
            p.build_bytes_vec_compressed().unwrap()
        };
        let name = std::env::var("MDNSVIEW_LIVE_IFACE").unwrap_or_else(|_| "WLAN".into());
        let iface = list_interfaces().unwrap().into_iter().find(|i| i.name == name).unwrap();
        let sockets = [MdnsSocket::open_v4().unwrap(), MdnsSocket::open_v6().unwrap()];
        let send = |ttl| {
            for s in &sockets {
                s.join(&iface).unwrap();
                s.send_to_group(&announcement(ttl), &iface).unwrap();
                s.leave(&iface).unwrap();
            }
        };
        send(120);

        let (tx, mut rx) = mpsc::channel(4096);
        let task = tokio::spawn(Box::new(WinDnsBackend).run(tx));
        let instance = instance_in(&format!("{label}.{TYPE}"), &DnsName::parse(TYPE));
        let (mut found, mut removed) = (false, false);
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut goodbye_at = None;
        while Instant::now() < deadline && !removed {
            if goodbye_at.is_some_and(|t| Instant::now() >= t) {
                println!("sending goodbye");
                send(0);
                goodbye_at = Some(deadline); // only once
            }
            let Ok(obs) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await else {
                continue;
            };
            match obs {
                Some(Observation::Record(r)) if r.name == instance || r.data == RData::Ptr(instance.clone()) => {
                    println!("{} {:?}", r.name, r.data);
                    found = true;
                    goodbye_at.get_or_insert(Instant::now() + Duration::from_secs(2));
                }
                Some(Observation::SystemRemoved { instance: i, .. }) if i == instance => {
                    println!("removed {i}");
                    removed = true;
                }
                Some(_) => {}
                None => break,
            }
        }
        drop(rx);
        let _ = task.await;
        assert!(found, "the announced instance was not reported");
        assert!(removed, "the goodbye was not reported");
    }

    #[tokio::test]
    async fn missing_api_is_reported_unavailable() {
        let (tx, mut rx) = mpsc::channel(4);
        run_with(|| Err(API_MISSING.into()), tx).await;
        assert!(matches!(
            rx.recv().await,
            Some(Observation::SourceStatus {
                source: Source::System,
                state: SourceState::Unavailable(reason)
            }) if reason == API_MISSING
        ));
    }
}
