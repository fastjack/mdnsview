//! mDNSResponder through the `dns_sd` API in libSystem: service type browser
//! → service browsers → resolvers → address lookups, all on one shared
//! connection to the daemon.
//!
//! `DNSServiceRef`s are not thread-safe, so the whole backend runs on one
//! blocking thread. The C callbacks only queue [`Event`]s; the [`Tracker`]
//! turns them into observations and into operations to start or stop.

use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, c_char, c_void};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ptr;
use std::rc::Rc;
use std::time::SystemTime;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use super::names::{domain_or_local, instance_name, parse_presentation};
use super::scopes::{InUse, ScopeBook};
use super::{BoxFuture, SystemBackend};
use crate::model::record::{DnsName, Observation, ObservedRecord, RData, Source, SourceState};
use crate::wire::monitor::InterfaceSet;

/// How often the loop checks whether the observation channel has closed.
const POLL_TIMEOUT_MS: i32 = 250;

/// Declarations from the macOS SDK's `dns_sd.h`. `DNSSD_API` is the default
/// C ABI on Apple platforms.
#[allow(non_snake_case)]
mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    #[repr(C)]
    pub struct DNSServiceRefOpaque {
        _private: [u8; 0],
    }
    pub type DNSServiceRef = *mut DNSServiceRefOpaque;
    pub type DNSServiceFlags = u32;
    pub type DNSServiceProtocol = u32;
    pub type DNSServiceErrorType = i32;

    pub const FLAGS_ADD: DNSServiceFlags = 0x2;
    pub const FLAGS_SHARE_CONNECTION: DNSServiceFlags = 0x4000;
    pub const PROTOCOL_IPV4: DNSServiceProtocol = 0x1;
    pub const PROTOCOL_IPV6: DNSServiceProtocol = 0x2;
    pub const INTERFACE_ANY: u32 = 0;
    pub const ERR_NO_ERROR: DNSServiceErrorType = 0;
    pub const ERR_NO_SUCH_RECORD: DNSServiceErrorType = -65554;
    pub const ERR_SERVICE_NOT_RUNNING: DNSServiceErrorType = -65563;

    pub type BrowseReply = unsafe extern "C" fn(
        sd: DNSServiceRef,
        flags: DNSServiceFlags,
        interface: u32,
        error: DNSServiceErrorType,
        service_name: *const c_char,
        regtype: *const c_char,
        reply_domain: *const c_char,
        context: *mut c_void,
    );
    pub type ResolveReply = unsafe extern "C" fn(
        sd: DNSServiceRef,
        flags: DNSServiceFlags,
        interface: u32,
        error: DNSServiceErrorType,
        fullname: *const c_char,
        host_target: *const c_char,
        port: u16,
        txt_len: u16,
        txt: *const u8,
        context: *mut c_void,
    );
    pub type GetAddrInfoReply = unsafe extern "C" fn(
        sd: DNSServiceRef,
        flags: DNSServiceFlags,
        interface: u32,
        error: DNSServiceErrorType,
        hostname: *const c_char,
        address: *const libc::sockaddr,
        ttl: u32,
        context: *mut c_void,
    );

    unsafe extern "C" {
        pub fn DNSServiceCreateConnection(sd: *mut DNSServiceRef) -> DNSServiceErrorType;
        pub fn DNSServiceRefSockFD(sd: DNSServiceRef) -> c_int;
        pub fn DNSServiceProcessResult(sd: DNSServiceRef) -> DNSServiceErrorType;
        pub fn DNSServiceRefDeallocate(sd: DNSServiceRef);
        pub fn DNSServiceBrowse(
            sd: *mut DNSServiceRef,
            flags: DNSServiceFlags,
            interface: u32,
            regtype: *const c_char,
            domain: *const c_char,
            callback: BrowseReply,
            context: *mut c_void,
        ) -> DNSServiceErrorType;
        pub fn DNSServiceResolve(
            sd: *mut DNSServiceRef,
            flags: DNSServiceFlags,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            callback: ResolveReply,
            context: *mut c_void,
        ) -> DNSServiceErrorType;
        pub fn DNSServiceGetAddrInfo(
            sd: *mut DNSServiceRef,
            flags: DNSServiceFlags,
            interface: u32,
            protocol: DNSServiceProtocol,
            hostname: *const c_char,
            callback: GetAddrInfoReply,
            context: *mut c_void,
        ) -> DNSServiceErrorType;
    }
}

pub struct DnsSdBackend;

impl SystemBackend for DnsSdBackend {
    fn name(&self) -> &'static str {
        "mDNSResponder"
    }

    fn run(
        self: Box<Self>,
        observations: mpsc::Sender<Observation>,
        interfaces: watch::Receiver<InterfaceSet>,
    ) -> BoxFuture {
        Box::pin(async move {
            let tx = observations.clone();
            let state = tokio::task::spawn_blocking(move || run_blocking(&tx, interfaces))
                .await
                .unwrap_or_else(|e| Some(SourceState::Unavailable(format!("mDNSResponder backend failed: {e}"))));
            if let Some(state) = state {
                if let SourceState::Unavailable(reason) = &state {
                    warn!("mDNSResponder unavailable: {reason}");
                }
                let _ = observations
                    .send(Observation::SourceStatus {
                        source: Source::System,
                        state,
                    })
                    .await;
            }
        })
    }
}

/// Identifies one daemon operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum OpKey {
    /// The `_services._dns-sd._udp` browse for service types.
    Types,
    /// Browse for instances of a service type (lowercased, e.g. `_ipp._tcp`).
    Browse(String),
    /// Resolve an instance (name key) on one interface.
    Resolve(String, u32),
    /// Address lookup for a host (name key) on one interface.
    Address(String, u32),
}

/// What a callback reported.
#[derive(Debug, Clone, PartialEq)]
enum Event {
    /// A service type from the `_services._dns-sd._udp` browse, which the
    /// daemon reports as instance `_ipp` of type `_tcp.local.`.
    TypeFound { name: String, regtype: String },
    Instance {
        add: bool,
        ifindex: u32,
        name: String,
        regtype: String,
        domain: String,
    },
    Resolved {
        instance: DnsName,
        ifindex: u32,
        host: String,
        port: u16,
        txt: Vec<u8>,
    },
    Address { ifindex: u32, host: String, addr: IpAddr },
    Failed { key: OpKey, code: i32 },
}

/// An operation for the driver to start or stop.
#[derive(Debug, Clone, PartialEq)]
enum Command {
    Browse {
        regtype: String,
    },
    Resolve {
        name: String,
        regtype: String,
        domain: String,
        ifindex: u32,
        instance: DnsName,
    },
    Address {
        host: String,
        ifindex: u32,
    },
    Stop(OpKey),
}

/// Daemon-side bookkeeping, independent of the FFI so it can be tested.
struct Tracker {
    types: HashSet<String>,
    /// What the daemon reports per instance and interface, limited to the
    /// interfaces in use.
    book: ScopeBook<u32>,
    in_use: InUse,
    /// (instance key, interface) → key of the host it last resolved to.
    resolvers: HashMap<(String, u32), Option<String>>,
    /// Running address lookups: (host key, interface).
    lookups: HashSet<(String, u32)>,
    names: HashMap<u32, Option<String>>,
    interface_name: Box<dyn Fn(u32) -> Option<String>>,
}

impl Tracker {
    fn new(interface_name: Box<dyn Fn(u32) -> Option<String>>) -> Self {
        Self {
            types: HashSet::new(),
            book: ScopeBook::default(),
            in_use: InUse::all(),
            resolvers: HashMap::new(),
            lookups: HashSet::new(),
            names: HashMap::new(),
            interface_name,
        }
    }

    fn iface(&mut self, index: u32) -> Option<String> {
        let lookup = &self.interface_name;
        self.names.entry(index).or_insert_with(|| lookup(index)).clone()
    }

    fn handle(&mut self, event: Event, out: &mut Vec<Observation>, commands: &mut Vec<Command>) {
        match event {
            Event::TypeFound { name, regtype } => {
                let Some((service_type, domain)) = service_type(&name, &regtype) else {
                    return;
                };
                let meta = DnsName::from_labels(
                    ["_services", "_dns-sd", "_udp"]
                        .into_iter()
                        .map(String::from)
                        .chain(domain.labels().iter().cloned()),
                );
                let short = DnsName::from_labels(service_type.labels()[..2].to_vec());
                if self.types.insert(short.key().to_string()) {
                    commands.push(Command::Browse {
                        regtype: short.to_string(),
                    });
                }
                out.push(observe(meta, RData::Ptr(service_type), None));
            }
            Event::Instance {
                add,
                ifindex,
                name,
                regtype,
                domain,
            } => {
                let instance = instance_name(&name, &regtype, &domain);
                let key = (instance.key().to_string(), ifindex);
                if add {
                    if let Entry::Vacant(e) = self.resolvers.entry(key) {
                        e.insert(None);
                        commands.push(Command::Resolve {
                            name,
                            regtype,
                            domain,
                            ifindex,
                            instance: instance.clone(),
                        });
                    }
                    let iface = self.iface(ifindex);
                    let ptr = observe(instance.parent(), RData::Ptr(instance.clone()), iface.clone());
                    out.extend(self.book.add(&instance, ifindex, iface, vec![ptr], &self.in_use));
                    return;
                }
                self.stop_resolver(&key, commands);
                out.extend(self.book.remove(&instance, ifindex, &self.in_use));
            }
            Event::Resolved {
                instance,
                ifindex,
                host,
                port,
                txt,
            } => {
                let key = (instance.key().to_string(), ifindex);
                let host_name = parse_presentation(&host);
                let host_key = host_name.key().to_string();
                let Some(previous) = self.resolvers.get_mut(&key) else {
                    return; // stopped since the reply was queued
                };
                let previous = previous.replace(host_key.clone());
                if let Some(previous) = previous.filter(|p| *p != host_key) {
                    self.stop_lookup_if_unused(previous, ifindex, commands);
                }
                if self.lookups.insert((host_key, ifindex)) {
                    commands.push(Command::Address { host, ifindex });
                }
                let iface = self.iface(ifindex);
                let records = vec![
                    observe(
                        instance.clone(),
                        RData::Srv {
                            priority: 0,
                            weight: 0,
                            port,
                            target: host_name,
                        },
                        iface.clone(),
                    ),
                    observe(instance.clone(), RData::Txt(split_txt(&txt)), iface.clone()),
                ];
                out.extend(self.book.add(&instance, ifindex, iface, records, &self.in_use));
            }
            Event::Address { ifindex, host, addr } => {
                let host = parse_presentation(&host);
                let data = match addr {
                    IpAddr::V4(a) => RData::A(a),
                    IpAddr::V6(a) => RData::Aaaa(a),
                };
                let iface = self.iface(ifindex);
                let record = observe(host.clone(), data, iface.clone());
                out.extend(self.book.host(&host, ifindex, iface, vec![record], &self.in_use));
            }
            Event::Failed { key, code } => {
                debug!("mDNSResponder operation {key:?} failed with error {code}");
                self.forget(&key);
                commands.push(Command::Stop(key));
            }
        }
    }

    /// The interfaces in use changed: release and replay accordingly.
    fn set_in_use(&mut self, in_use: InUse) -> Vec<Observation> {
        let out = self.book.set_in_use(&self.in_use, &in_use);
        self.in_use = in_use;
        out
    }

    /// Drops the bookkeeping for an operation that failed or could not start,
    /// so a later report can start it again.
    fn forget(&mut self, key: &OpKey) {
        match key {
            OpKey::Types => {}
            OpKey::Browse(t) => {
                self.types.remove(t);
            }
            OpKey::Resolve(instance, ifindex) => {
                self.resolvers.remove(&(instance.clone(), *ifindex));
            }
            OpKey::Address(host, ifindex) => {
                self.lookups.remove(&(host.clone(), *ifindex));
            }
        }
    }

    fn stop_resolver(&mut self, key: &(String, u32), commands: &mut Vec<Command>) {
        let Some(host) = self.resolvers.remove(key) else {
            return;
        };
        commands.push(Command::Stop(OpKey::Resolve(key.0.clone(), key.1)));
        if let Some(host) = host {
            self.stop_lookup_if_unused(host, key.1, commands);
        }
    }

    fn stop_lookup_if_unused(&mut self, host: String, ifindex: u32, commands: &mut Vec<Command>) {
        let in_use = self
            .resolvers
            .iter()
            .any(|((_, i), h)| *i == ifindex && h.as_deref() == Some(host.as_str()));
        if !in_use && self.lookups.remove(&(host.clone(), ifindex)) {
            commands.push(Command::Stop(OpKey::Address(host, ifindex)));
        }
    }
}

/// The type name and domain from a `_services._dns-sd._udp` browse result:
/// `_ipp` + `_tcp.local.` → (`_ipp._tcp.local`, `local`).
fn service_type(name: &str, regtype: &str) -> Option<(DnsName, DnsName)> {
    let parsed = DnsName::parse(regtype);
    let (proto, rest) = parsed.labels().split_first()?;
    if !name.starts_with('_') || !(proto.eq_ignore_ascii_case("_tcp") || proto.eq_ignore_ascii_case("_udp")) {
        return None;
    }
    let domain = domain_or_local(&rest.join("."));
    let labels = [name.to_string(), proto.clone()]
        .into_iter()
        .chain(domain.labels().iter().cloned());
    Some((DnsName::from_labels(labels), domain))
}

/// Splits TXT rdata into its length-prefixed strings, the same way the wire
/// codec does. An empty record is one empty string.
fn split_txt(data: &[u8]) -> Vec<Vec<u8>> {
    if data.is_empty() {
        return vec![Vec::new()];
    }
    let mut strings = Vec::new();
    let mut rest = data;
    while let Some((&len, tail)) = rest.split_first() {
        let len = usize::from(len).min(tail.len());
        strings.push(tail[..len].to_vec());
        rest = &tail[len..];
    }
    strings
}

/// The daemon passes ports in network byte order.
fn port_from_network(raw: u16) -> u16 {
    u16::from_be(raw)
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

fn interface_name(index: u32) -> Option<String> {
    let mut buf = [0 as c_char; libc::IF_NAMESIZE];
    // SAFETY: `buf` has room for IF_NAMESIZE bytes, as `if_indextoname` requires.
    let name = unsafe { libc::if_indextoname(index, buf.as_mut_ptr()) };
    if name.is_null() {
        return None;
    }
    // SAFETY: on success the buffer holds a NUL-terminated name.
    Some(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
}

// ---- FFI driver ------------------------------------------------------------

/// What a callback needs: where to queue events and which operation it is.
struct OpContext {
    queue: Rc<RefCell<Vec<Event>>>,
    key: OpKey,
    instance: Option<DnsName>,
}

/// A running operation on the shared connection. Dropping it cancels it.
struct Op {
    sd: ffi::DNSServiceRef,
    // Freed after `sd` is deallocated, so no callback can see it dangling.
    _context: Box<OpContext>,
}

impl Drop for Op {
    fn drop(&mut self) {
        // SAFETY: `sd` is a live sub-reference of the shared connection, which
        // `Session` keeps alive until every `Op` is gone.
        unsafe { ffi::DNSServiceRefDeallocate(self.sd) }
    }
}

struct Session {
    // Declared before `conn`: all operations are cancelled before the
    // connection that carries them.
    ops: HashMap<OpKey, Op>,
    conn: Connection,
    queue: Rc<RefCell<Vec<Event>>>,
}

struct Connection(ffi::DNSServiceRef);

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: created by `DNSServiceCreateConnection` and deallocated once.
        unsafe { ffi::DNSServiceRefDeallocate(self.0) }
    }
}

impl Session {
    fn connect() -> Result<Self, i32> {
        let mut sd = ptr::null_mut();
        // SAFETY: `sd` is a valid out-pointer.
        let err = unsafe { ffi::DNSServiceCreateConnection(&mut sd) };
        if err != ffi::ERR_NO_ERROR {
            return Err(err);
        }
        Ok(Self {
            ops: HashMap::new(),
            conn: Connection(sd),
            queue: Rc::default(),
        })
    }

    /// Starts an operation on the shared connection. `call` receives the
    /// sub-reference to fill in and the callback context.
    fn start(
        &mut self,
        key: OpKey,
        instance: Option<DnsName>,
        call: impl FnOnce(*mut ffi::DNSServiceRef, *mut c_void) -> i32,
    ) -> Result<(), i32> {
        let context = Box::new(OpContext {
            queue: self.queue.clone(),
            key: key.clone(),
            instance,
        });
        // With kDNSServiceFlagsShareConnection the reference starts as a copy
        // of the shared connection and is replaced by the sub-reference.
        let mut sd = self.conn.0;
        let err = call(&mut sd, &*context as *const OpContext as *mut c_void);
        if err != ffi::ERR_NO_ERROR {
            return Err(err);
        }
        self.ops.insert(key, Op { sd, _context: context });
        Ok(())
    }

    fn execute(&mut self, command: Command) -> Result<(), (OpKey, i32)> {
        let flags = ffi::FLAGS_SHARE_CONNECTION;
        match command {
            Command::Stop(key) => {
                self.ops.remove(&key);
                Ok(())
            }
            Command::Browse { regtype } => {
                let key = OpKey::Browse(regtype.to_ascii_lowercase());
                let c_type = c_string(&regtype);
                let c_domain = c_string("local.");
                self.start(key.clone(), None, |sd, ctx| unsafe {
                    // SAFETY: valid out-pointer, NUL-terminated strings, and a
                    // context that lives as long as the operation.
                    ffi::DNSServiceBrowse(
                        sd,
                        flags,
                        ffi::INTERFACE_ANY,
                        c_type.as_ptr(),
                        c_domain.as_ptr(),
                        on_browse,
                        ctx,
                    )
                })
                .map_err(|e| (key, e))
            }
            Command::Resolve {
                name,
                regtype,
                domain,
                ifindex,
                instance,
            } => {
                let key = OpKey::Resolve(instance.key().to_string(), ifindex);
                let (c_name, c_type, c_domain) = (c_string(&name), c_string(&regtype), c_string(&domain));
                self.start(key.clone(), Some(instance), |sd, ctx| unsafe {
                    // SAFETY: as for the browse above.
                    ffi::DNSServiceResolve(
                        sd,
                        flags,
                        ifindex,
                        c_name.as_ptr(),
                        c_type.as_ptr(),
                        c_domain.as_ptr(),
                        on_resolve,
                        ctx,
                    )
                })
                .map_err(|e| (key, e))
            }
            Command::Address { host, ifindex } => {
                let key = OpKey::Address(parse_presentation(&host).key().to_string(), ifindex);
                let c_host = c_string(&host);
                self.start(key.clone(), None, |sd, ctx| unsafe {
                    // SAFETY: as for the browse above.
                    ffi::DNSServiceGetAddrInfo(
                        sd,
                        flags,
                        ifindex,
                        ffi::PROTOCOL_IPV4 | ffi::PROTOCOL_IPV6,
                        c_host.as_ptr(),
                        on_address,
                        ctx,
                    )
                })
                .map_err(|e| (key, e))
            }
        }
    }
}

fn c_string(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// # Safety
/// `p` is null or points to a NUL-terminated string.
unsafe fn text(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: guaranteed by the caller.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// # Safety
/// `context` is the `OpContext` registered with the operation.
unsafe fn push(context: *mut c_void, event: Event) {
    // SAFETY: guaranteed by the caller; callbacks run on the driver thread.
    let context = unsafe { &*(context as *const OpContext) };
    context.queue.borrow_mut().push(event);
}

unsafe extern "C" fn on_browse(
    _sd: ffi::DNSServiceRef,
    flags: ffi::DNSServiceFlags,
    ifindex: u32,
    error: ffi::DNSServiceErrorType,
    name: *const c_char,
    regtype: *const c_char,
    domain: *const c_char,
    context: *mut c_void,
) {
    // SAFETY: the daemon passes NUL-terminated strings and our context.
    unsafe {
        let key = (*(context as *const OpContext)).key.clone();
        let event = if error != ffi::ERR_NO_ERROR {
            Event::Failed { key, code: error }
        } else if key == OpKey::Types {
            Event::TypeFound {
                name: text(name),
                regtype: text(regtype),
            }
        } else {
            Event::Instance {
                add: flags & ffi::FLAGS_ADD != 0,
                ifindex,
                name: text(name),
                regtype: text(regtype),
                domain: text(domain),
            }
        };
        push(context, event);
    }
}

unsafe extern "C" fn on_resolve(
    _sd: ffi::DNSServiceRef,
    _flags: ffi::DNSServiceFlags,
    ifindex: u32,
    error: ffi::DNSServiceErrorType,
    _fullname: *const c_char,
    host: *const c_char,
    port: u16,
    txt_len: u16,
    txt: *const u8,
    context: *mut c_void,
) {
    // SAFETY: the daemon passes a NUL-terminated host, `txt_len` bytes at
    // `txt`, and our context.
    unsafe {
        let ctx = &*(context as *const OpContext);
        let event = match (&ctx.instance, error) {
            (Some(instance), ffi::ERR_NO_ERROR) => Event::Resolved {
                instance: instance.clone(),
                ifindex,
                host: text(host),
                port: port_from_network(port),
                txt: if txt.is_null() {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(txt, usize::from(txt_len)).to_vec()
                },
            },
            _ => Event::Failed {
                key: ctx.key.clone(),
                code: error,
            },
        };
        push(context, event);
    }
}

unsafe extern "C" fn on_address(
    _sd: ffi::DNSServiceRef,
    flags: ffi::DNSServiceFlags,
    ifindex: u32,
    error: ffi::DNSServiceErrorType,
    host: *const c_char,
    address: *const libc::sockaddr,
    _ttl: u32,
    context: *mut c_void,
) {
    // A negative answer (e.g. no AAAA record) is not a failure.
    if error == ffi::ERR_NO_SUCH_RECORD || flags & ffi::FLAGS_ADD == 0 {
        return;
    }
    // SAFETY: the daemon passes a NUL-terminated host, a sockaddr matching
    // its family, and our context.
    unsafe {
        let event = if error != ffi::ERR_NO_ERROR {
            Event::Failed {
                key: (*(context as *const OpContext)).key.clone(),
                code: error,
            }
        } else {
            let Some(addr) = sockaddr_ip(address) else {
                return;
            };
            Event::Address {
                ifindex,
                host: text(host),
                addr,
            }
        };
        push(context, event);
    }
}

/// # Safety
/// `sa` is null or points to a sockaddr of the size its family implies.
unsafe fn sockaddr_ip(sa: *const libc::sockaddr) -> Option<IpAddr> {
    if sa.is_null() {
        return None;
    }
    // SAFETY: guaranteed by the caller.
    unsafe {
        match i32::from((*sa).sa_family) {
            libc::AF_INET => {
                let sin = &*(sa as *const libc::sockaddr_in);
                Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr))))
            }
            libc::AF_INET6 => {
                let sin6 = &*(sa as *const libc::sockaddr_in6);
                Some(IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.s6_addr)))
            }
            _ => None,
        }
    }
}

/// Runs the backend until the daemon goes away or the channel closes.
/// Returns the final state to report, or `None` if nobody is listening.
fn run_blocking(
    observations: &mpsc::Sender<Observation>,
    mut interfaces: watch::Receiver<InterfaceSet>,
) -> Option<SourceState> {
    let unavailable = |what: &str, code: i32| {
        Some(SourceState::Unavailable(if code == ffi::ERR_SERVICE_NOT_RUNNING {
            "mDNSResponder is not running".into()
        } else {
            format!("{what} (dns_sd error {code})")
        }))
    };
    let mut session = match Session::connect() {
        Ok(s) => s,
        Err(code) => return unavailable("cannot connect to mDNSResponder", code),
    };
    let meta_type = c_string("_services._dns-sd._udp");
    let domain = c_string("local.");
    let started = session.start(OpKey::Types, None, |sd, ctx| unsafe {
        // SAFETY: valid out-pointer, NUL-terminated strings, and a context
        // that lives as long as the operation.
        ffi::DNSServiceBrowse(
            sd,
            ffi::FLAGS_SHARE_CONNECTION,
            ffi::INTERFACE_ANY,
            meta_type.as_ptr(),
            domain.as_ptr(),
            on_browse,
            ctx,
        )
    });
    if let Err(code) = started {
        return unavailable("cannot browse service types", code);
    }
    info!("connected to mDNSResponder");
    let active = Observation::SourceStatus {
        source: Source::System,
        state: SourceState::Active,
    };
    if observations.blocking_send(active).is_err() {
        return None;
    }

    // SAFETY: the connection is live.
    let fd = unsafe { ffi::DNSServiceRefSockFD(session.conn.0) };
    let mut tracker = Tracker::new(Box::new(interface_name));
    tracker.in_use = InUse::of(&interfaces.borrow_and_update());
    loop {
        if observations.is_closed() {
            return None;
        }
        // Checked at least every POLL_TIMEOUT_MS, well within a second.
        if interfaces.has_changed().unwrap_or(false) {
            let in_use = InUse::of(&interfaces.borrow_and_update());
            for obs in tracker.set_in_use(in_use) {
                if observations.blocking_send(obs).is_err() {
                    return None;
                }
            }
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut pfd, 1, POLL_TIMEOUT_MS) };
        if ready < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Some(SourceState::Unavailable(format!("poll on mDNSResponder socket failed: {e}")));
        }
        if ready == 0 {
            continue;
        }
        // SAFETY: the connection is live; callbacks run on this thread.
        let err = unsafe { ffi::DNSServiceProcessResult(session.conn.0) };
        if err != ffi::ERR_NO_ERROR {
            return unavailable("mDNSResponder went away", err);
        }

        let mut out = Vec::new();
        let mut commands = Vec::new();
        loop {
            let events = std::mem::take(&mut *session.queue.borrow_mut());
            if events.is_empty() && commands.is_empty() {
                break;
            }
            for event in events {
                if let Event::Failed { code, .. } = event
                    && code == ffi::ERR_SERVICE_NOT_RUNNING
                {
                    return unavailable("mDNSResponder went away", code);
                }
                tracker.handle(event, &mut out, &mut commands);
            }
            for command in commands.drain(..) {
                if let Err((key, code)) = session.execute(command) {
                    debug!("cannot start {key:?}: dns_sd error {code}");
                    tracker.forget(&key);
                }
            }
        }
        for obs in out {
            if observations.blocking_send(obs).is_err() {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> Tracker {
        Tracker::new(Box::new(|i| Some(format!("en{i}"))))
    }

    fn handle(t: &mut Tracker, event: Event) -> (Vec<Observation>, Vec<Command>) {
        let (mut out, mut commands) = (Vec::new(), Vec::new());
        t.handle(event, &mut out, &mut commands);
        (out, commands)
    }

    fn instance_event(add: bool, ifindex: u32) -> Event {
        Event::Instance {
            add,
            ifindex,
            name: "Mr. Printer".into(),
            regtype: "_ipp._tcp.".into(),
            domain: "local.".into(),
        }
    }

    fn removed(out: &[Observation]) -> bool {
        out.iter().any(|o| matches!(o, Observation::SystemRemoved { .. }))
    }

    #[test]
    fn service_type_from_meta_browse_result() {
        let (t, domain) = service_type("_ipp", "_tcp.local.").unwrap();
        assert_eq!(t.to_string(), "_ipp._tcp.local");
        assert!(t.is_service_type());
        assert_eq!(domain.to_string(), "local");
        assert!(service_type("ipp", "_tcp.local.").is_none());
        assert!(service_type("_ipp", "_xyz.local.").is_none());
    }

    #[test]
    fn txt_is_split_like_the_wire_codec() {
        assert_eq!(split_txt(b"\x09txtvers=1\x04note"), vec![b"txtvers=1".to_vec(), b"note".to_vec()]);
        assert_eq!(split_txt(b"\x00"), vec![Vec::<u8>::new()]);
        assert_eq!(split_txt(b""), vec![Vec::<u8>::new()]);
        // A length running past the end keeps what is there.
        assert_eq!(split_txt(b"\x05ab"), vec![b"ab".to_vec()]);
    }

    #[test]
    fn ports_arrive_in_network_byte_order() {
        assert_eq!(port_from_network(631u16.to_be()), 631);
    }

    #[test]
    fn service_types_start_one_browser_each() {
        let mut t = tracker();
        let found = Event::TypeFound {
            name: "_ipp".into(),
            regtype: "_tcp.local.".into(),
        };
        let (out, commands) = handle(&mut t, found.clone());
        assert_eq!(commands, vec![Command::Browse { regtype: "_ipp._tcp".into() }]);
        let Observation::Record(r) = &out[0] else { panic!() };
        assert_eq!(r.name.to_string(), "_services._dns-sd._udp.local");
        assert_eq!(r.data, RData::Ptr(DnsName::parse("_ipp._tcp.local")));
        let (_, again) = handle(&mut t, found);
        assert!(again.is_empty());
    }

    #[test]
    fn removal_waits_for_the_last_interface() {
        let mut t = tracker();
        let (out, commands) = handle(&mut t, instance_event(true, 4));
        assert!(matches!(&commands[..], [Command::Resolve { ifindex: 4, .. }]));
        let Observation::Record(r) = &out[0] else { panic!() };
        assert_eq!(r.name.to_string(), "_ipp._tcp.local");
        assert_eq!(r.iface.as_deref(), Some("en4"));
        let (_, commands) = handle(&mut t, instance_event(true, 5));
        assert!(matches!(&commands[..], [Command::Resolve { ifindex: 5, .. }]));

        let key = instance_name("Mr. Printer", "_ipp._tcp.", "local.").key().to_string();
        let (out, commands) = handle(&mut t, instance_event(false, 4));
        assert!(!removed(&out));
        assert_eq!(commands, vec![Command::Stop(OpKey::Resolve(key.clone(), 4))]);
        let (out, commands) = handle(&mut t, instance_event(false, 5));
        assert!(removed(&out));
        assert_eq!(commands, vec![Command::Stop(OpKey::Resolve(key, 5))]);
    }

    #[test]
    fn results_on_interfaces_not_in_use_are_held_back() {
        let mut t = tracker();
        t.in_use = InUse::names(&["en4"]);
        let (out, commands) = handle(&mut t, instance_event(true, 9));
        assert!(out.is_empty(), "en9 not in use: {out:?}");
        assert!(matches!(&commands[..], [Command::Resolve { ifindex: 9, .. }]), "still resolved, for a later replay");
        let address = Event::Address {
            ifindex: 9,
            host: "printer.local.".into(),
            addr: "192.168.1.40".parse().unwrap(),
        };
        assert!(handle(&mut t, address).0.is_empty());

        let out = t.set_in_use(InUse::names(&["en4", "en9"]));
        let types: Vec<u16> = out
            .iter()
            .map(|o| match o {
                Observation::Record(r) => r.data.rtype(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(types, vec![crate::model::record::rtype::PTR, crate::model::record::rtype::A]);
    }

    #[test]
    fn deselecting_the_interface_releases_the_instance() {
        let mut t = tracker();
        t.in_use = InUse::names(&["en4"]);
        handle(&mut t, instance_event(true, 4));
        let out = t.set_in_use(InUse::names(&["en5"]));
        assert!(matches!(&out[..], [Observation::SystemReleased { .. }]));
        let (out, _) = handle(&mut t, instance_event(false, 4));
        assert!(!removed(&out), "already released");
    }

    #[test]
    fn resolve_reports_srv_txt_and_starts_one_address_lookup() {
        let mut t = tracker();
        let instance = instance_name("Mr. Printer", "_ipp._tcp.", "local.");
        handle(&mut t, instance_event(true, 4));
        let resolved = Event::Resolved {
            instance: instance.clone(),
            ifindex: 4,
            host: "BRN30055C1A4C01.local.".into(),
            port: 631,
            txt: b"\x09txtvers=1".to_vec(),
        };
        let (out, commands) = handle(&mut t, resolved.clone());
        assert_eq!(
            commands,
            vec![Command::Address {
                host: "BRN30055C1A4C01.local.".into(),
                ifindex: 4
            }]
        );
        let data: Vec<_> = out
            .iter()
            .map(|o| match o {
                Observation::Record(r) => r.data.clone(),
                _ => panic!(),
            })
            .collect();
        assert_eq!(
            data,
            vec![
                RData::Srv {
                    priority: 0,
                    weight: 0,
                    port: 631,
                    target: DnsName::parse("BRN30055C1A4C01.local"),
                },
                RData::Txt(vec![b"txtvers=1".to_vec()]),
            ]
        );
        let (_, again) = handle(&mut t, resolved);
        assert!(again.is_empty(), "lookup already running");

        // Removing the only instance on that host stops its lookup too.
        let (_, commands) = handle(&mut t, instance_event(false, 4));
        let host_key = DnsName::parse("BRN30055C1A4C01.local").key().to_string();
        assert!(commands.contains(&Command::Stop(OpKey::Address(host_key, 4))));
    }

    #[test]
    fn addresses_become_a_and_aaaa_records() {
        let mut t = tracker();
        let (out, _) = handle(
            &mut t,
            Event::Address {
                ifindex: 4,
                host: "BRN30055C1A4C01.local.".into(),
                addr: "192.168.4.123".parse().unwrap(),
            },
        );
        let Observation::Record(r) = &out[0] else { panic!() };
        assert_eq!(r.data, RData::A(Ipv4Addr::new(192, 168, 4, 123)));
        assert_eq!(r.source, Source::System);
        assert_eq!(r.ttl, None);
    }

    #[test]
    fn failed_operations_can_start_again() {
        let mut t = tracker();
        handle(&mut t, instance_event(true, 4));
        let key = OpKey::Resolve(instance_name("Mr. Printer", "_ipp._tcp.", "local.").key().to_string(), 4);
        let (_, commands) = handle(&mut t, Event::Failed { key: key.clone(), code: -65537 });
        assert_eq!(commands, vec![Command::Stop(key)]);
        let (_, commands) = handle(&mut t, instance_event(true, 4));
        assert!(matches!(&commands[..], [Command::Resolve { .. }]));
    }

    /// Runs against the local mDNSResponder:
    /// `cargo test live_dnssd -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs mDNSResponder and a LAN"]
    async fn live_dnssd() {
        let (tx, mut rx) = mpsc::channel(4096);
        let (_sets, sets_rx) = watch::channel(crate::wire::monitor::Monitor::default_set());
        let task = tokio::spawn(Box::new(DnsSdBackend).run(tx, sets_rx));
        let deadline = Instant::now() + std::time::Duration::from_secs(4);
        let mut seen = HashSet::new();
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
}
