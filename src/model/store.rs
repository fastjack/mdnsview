//! The record store: sole owner of all records, run as one task.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, MissedTickBehavior};

use super::record::{
    DnsName, Observation, ObservedRecord, QueryRequest, RData, RecordKey, Source, Sources, rtype,
};
use super::snapshot::{DeriveInput, Snapshot, Status, derive};

/// How long a superseded record lives after a cache-flush (RFC 6762 §10.2).
const CACHE_FLUSH_GRACE: Duration = Duration::from_secs(1);
/// Minimum interval between refinement rounds for the same name.
const REFINE_INTERVAL: Duration = Duration::from_secs(30);
/// Delays before each retry of a refinement request that is still missing
/// records. Longer than the scheduler's 1 s dedup, so no retry is swallowed.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(8),
    Duration::from_secs(30),
];
/// Snapshots are published at most this often.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(250);
/// Liveness and garbage collection are re-evaluated this often.
const GC_INTERVAL: Duration = Duration::from_secs(1);

/// What a refined name is, which decides the record types it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefineKind {
    Instance,
    Host,
}

/// One refinement round for a name: the first request and its retries.
struct Refinement {
    kind: RefineKind,
    /// When the round started.
    requested: Instant,
    retries: usize,
    /// `None` once every retry has been sent.
    next_retry: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredRecord {
    pub name: DnsName,
    pub data: RData,
    pub sources: Sources,
    /// Original TTL of the last wire copy.
    pub ttl: Option<u32>,
    /// Expiry of the wire copy.
    pub expires: Option<Instant>,
    /// Last time a wire copy was received.
    pub wire_seen: Option<Instant>,
    /// Reported by the OS daemon and not yet removed by it.
    pub held_by_system: bool,
    pub cache_flush: bool,
    pub src_addr: Option<IpAddr>,
    pub iface: Option<String>,
    pub first_seen: Instant,
    pub first_seen_wall: SystemTime,
    pub last_seen: Instant,
    pub last_seen_wall: SystemTime,
    pub goodbye: Option<Instant>,
}

impl StoredRecord {
    pub fn is_live(&self, now: Instant) -> bool {
        self.goodbye.is_none() && (self.held_by_system || self.expires.is_some_and(|e| e > now))
    }
}

pub struct Store {
    records: HashMap<RecordKey, StoredRecord>,
    by_owner: HashMap<String, HashSet<RecordKey>>,
    timeout: Duration,
    retained: HashSet<String>,
    status: Status,
    refinements: HashMap<DnsName, Refinement>,
}

impl Store {
    pub fn new(timeout: Duration) -> Self {
        Self {
            records: HashMap::new(),
            by_owner: HashMap::new(),
            timeout,
            retained: HashSet::new(),
            status: Status::default(),
            refinements: HashMap::new(),
        }
    }

    /// Applies one observation; returns queries the wire backend should send.
    pub fn ingest(&mut self, obs: Observation) -> Vec<QueryRequest> {
        match obs {
            Observation::Record(rec) => self.ingest_record(rec),
            Observation::SystemRemoved { instance, at } => {
                self.system_removed(&instance, at);
                Vec::new()
            }
            Observation::SourceStatus { source, state } => {
                match source {
                    Source::System => self.status.system = state,
                    Source::Wire => self.status.wire = state,
                }
                Vec::new()
            }
            Observation::Interfaces(names) => {
                self.status.interfaces = names;
                Vec::new()
            }
            Observation::WireSilent(silent) => {
                self.status.wire_silent = silent;
                Vec::new()
            }
        }
    }

    fn ingest_record(&mut self, rec: ObservedRecord) -> Vec<QueryRequest> {
        let key = rec.key();
        let at = rec.at;
        if rec.source == Source::Wire && rec.cache_flush && rec.ttl != Some(0) {
            self.apply_cache_flush(&key, rec.src_addr, at);
        }
        let entry = self.records.entry(key.clone()).or_insert_with(|| StoredRecord {
            name: rec.name.clone(),
            data: rec.data.clone(),
            sources: Sources::default(),
            ttl: None,
            expires: None,
            wire_seen: None,
            held_by_system: false,
            cache_flush: false,
            src_addr: None,
            iface: None,
            first_seen: at,
            first_seen_wall: rec.wall,
            last_seen: at,
            last_seen_wall: rec.wall,
            goodbye: None,
        });
        entry.sources.add(rec.source);
        entry.last_seen = at;
        entry.last_seen_wall = rec.wall;
        match rec.source {
            Source::Wire => {
                let ttl = rec.ttl.unwrap_or(0);
                entry.ttl = Some(ttl);
                entry.wire_seen = Some(at);
                entry.cache_flush = rec.cache_flush;
                entry.src_addr = rec.src_addr;
                entry.iface = rec.iface;
                if ttl == 0 {
                    entry.expires = Some(at);
                    entry.goodbye = Some(at);
                    entry.held_by_system = false;
                } else {
                    entry.expires = Some(at + Duration::from_secs(ttl.into()));
                    entry.goodbye = None;
                }
            }
            Source::System => {
                entry.held_by_system = true;
                entry.goodbye = None;
                if entry.iface.is_none() {
                    entry.iface = rec.iface;
                }
                if entry.src_addr.is_none() {
                    entry.src_addr = rec.src_addr;
                }
            }
        }
        self.by_owner
            .entry(key.name.key().to_string())
            .or_default()
            .insert(key.clone());
        self.refinements(&key.data, at)
    }

    /// RFC 6762 §10.2: other records of the same name and type that are
    /// older than one second expire one second from now. SRV and TXT records
    /// only flush their own host's records, so two hosts announcing the same
    /// instance name do not flush each other.
    fn apply_cache_flush(&mut self, key: &RecordKey, src_addr: Option<IpAddr>, at: Instant) {
        let rt = key.data.rtype();
        let Some(siblings) = self.by_owner.get(key.name.key()) else {
            return;
        };
        for sibling in siblings {
            if sibling == key || sibling.data.rtype() != rt {
                continue;
            }
            if let (RData::Srv { target, .. }, RData::Srv { target: other, .. }) = (&key.data, &sibling.data)
                && target != other
            {
                continue;
            }
            if let Some(r) = self.records.get_mut(sibling)
                && !(rt == rtype::TXT && r.src_addr.is_some() && r.src_addr != src_addr)
                && let Some(seen) = r.wire_seen
                && at.saturating_duration_since(seen) > CACHE_FLUSH_GRACE
            {
                let flush_at = at + CACHE_FLUSH_GRACE;
                r.expires = Some(r.expires.map_or(flush_at, |e| e.min(flush_at)));
            }
        }
    }

    fn has_wire(&self, name: &DnsName, rtypes: &[u16]) -> bool {
        self.by_owner.get(name.key()).is_some_and(|keys| {
            keys.iter().any(|k| {
                rtypes.contains(&k.data.rtype())
                    && self.records.get(k).is_some_and(|r| r.sources.wire)
            })
        })
    }

    /// Record types of `name` still to be fetched on the wire. An instance
    /// needs both SRV and TXT; a host needs an A or an AAAA record.
    fn missing(&self, kind: RefineKind, name: &DnsName) -> Vec<u16> {
        match kind {
            RefineKind::Instance => [rtype::SRV, rtype::TXT]
                .into_iter()
                .filter(|rt| !self.has_wire(name, &[*rt]))
                .collect(),
            RefineKind::Host if self.has_wire(name, &[rtype::A, rtype::AAAA]) => Vec::new(),
            RefineKind::Host => vec![rtype::A, rtype::AAAA],
        }
    }

    /// Asks for wire data about names we only know from the daemon, or
    /// whose SRV / TXT / address records have not been seen on the wire yet.
    fn refinements(&mut self, data: &RData, at: Instant) -> Vec<QueryRequest> {
        let (name, kind) = match data {
            RData::Ptr(instance) if instance.is_service_instance() => {
                (instance.clone(), RefineKind::Instance)
            }
            RData::Srv { target, .. } => (target.clone(), RefineKind::Host),
            _ => return Vec::new(),
        };
        let rtypes = self.missing(kind, &name);
        if rtypes.is_empty() {
            return Vec::new();
        }
        if let Some(round) = self.refinements.get(&name)
            && at.saturating_duration_since(round.requested) < REFINE_INTERVAL
        {
            return Vec::new();
        }
        self.refinements.insert(
            name.clone(),
            Refinement {
                kind,
                requested: at,
                retries: 0,
                next_retry: Some(at + RETRY_DELAYS[0]),
            },
        );
        vec![QueryRequest { name, rtypes }]
    }

    /// Retries due refinement requests for names still missing records.
    /// Names that are complete are dropped.
    pub fn due_refinements(&mut self, now: Instant) -> Vec<QueryRequest> {
        let due: Vec<(DnsName, RefineKind)> = self
            .refinements
            .iter()
            .filter(|(_, r)| r.next_retry.is_some_and(|t| t <= now))
            .map(|(name, r)| (name.clone(), r.kind))
            .collect();
        let mut out = Vec::new();
        for (name, kind) in due {
            let rtypes = self.missing(kind, &name);
            if rtypes.is_empty() {
                self.refinements.remove(&name);
                continue;
            }
            if let Some(round) = self.refinements.get_mut(&name) {
                round.retries += 1;
                round.next_retry = RETRY_DELAYS.get(round.retries).map(|d| now + *d);
                tracing::debug!("refinement retry {} for {name}: {rtypes:?}", round.retries);
            }
            out.push(QueryRequest { name, rtypes });
        }
        out
    }

    fn system_removed(&mut self, instance: &DnsName, at: Instant) {
        let type_name = instance.parent();
        let mut keys: Vec<RecordKey> = self.keys_owned(instance);
        keys.extend(
            self.keys_owned(&type_name)
                .into_iter()
                .filter(|k| matches!(&k.data, RData::Ptr(t) if t == instance)),
        );
        let mut hosts = Vec::new();
        for k in &keys {
            if let RData::Srv { target, .. } = &k.data {
                hosts.push(target.clone());
            }
            self.release_system(k, at);
        }
        // Address records stay held while another held SRV still points at the host.
        for host in hosts {
            let still_used = self.records.values().any(|r| {
                r.held_by_system && matches!(&r.data, RData::Srv { target, .. } if *target == host)
            });
            if !still_used {
                for k in self.keys_owned(&host) {
                    if k.data.address().is_some() {
                        self.release_system(&k, at);
                    }
                }
            }
        }
    }

    fn release_system(&mut self, key: &RecordKey, at: Instant) {
        if let Some(r) = self.records.get_mut(key)
            && r.held_by_system
        {
            r.held_by_system = false;
            r.last_seen = r.last_seen.max(at);
            if !r.expires.is_some_and(|e| e > at) {
                r.goodbye = Some(at);
            }
        }
    }

    fn keys_owned(&self, name: &DnsName) -> Vec<RecordKey> {
        self.by_owner
            .get(name.key())
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn set_retained(&mut self, ids: impl IntoIterator<Item = String>) {
        self.retained = ids.into_iter().collect();
    }

    pub fn snapshot(&self, now: Instant) -> Snapshot {
        derive(&DeriveInput {
            records: &self.records,
            by_owner: &self.by_owner,
            now,
            timeout: self.timeout,
            retained: &self.retained,
            status: &self.status,
        })
    }

    /// Removes devices and services not seen for longer than the timeout,
    /// unless they are retained by the UI selection.
    pub fn gc(&mut self, now: Instant) {
        let snap = self.snapshot(now);
        let mut remove: HashSet<RecordKey> = HashSet::new();
        let mut removed_services: HashSet<&str> = HashSet::new();
        let mut attributed: HashSet<&RecordKey> = HashSet::new();

        for device in &snap.devices {
            attributed.extend(device.host_keys.iter());
            for sid in &device.services {
                attributed.extend(snap.services[sid].own_keys.iter());
            }
            if device.gone {
                if device.retained {
                    continue;
                }
                remove.extend(device.host_keys.iter().cloned());
                removed_services.extend(
                    device
                        .services
                        .iter()
                        .filter(|sid| !snap.services[*sid].retained)
                        .map(String::as_str),
                );
                continue;
            }
            // A live device: drop host names that went silent on their own,
            // unless the selection still holds them; a held host keeps its
            // services so it stays part of the device.
            let mut held_services: HashSet<&String> = HashSet::new();
            for member in &device.members {
                let held = self.retained.contains(&member.id)
                    || member.services.iter().any(|sid| snap.services[sid].retained);
                if !member.gone {
                    continue;
                }
                if held {
                    held_services.extend(member.services.iter());
                } else {
                    remove.extend(member.host_keys.iter().cloned());
                    removed_services.extend(member.services.iter().map(String::as_str));
                }
            }
            for sid in &device.services {
                let service = &snap.services[sid];
                if service.gone && !service.retained && !held_services.contains(sid) {
                    removed_services.insert(sid);
                }
            }
        }
        // Records shared by services of one instance name, such as its PTR,
        // stay while a service that uses them stays.
        let kept: HashSet<&RecordKey> = snap
            .services
            .values()
            .filter(|s| !removed_services.contains(s.id.as_str()))
            .flat_map(|s| s.own_keys.iter())
            .collect();
        for sid in &removed_services {
            remove.extend(snap.services[*sid].own_keys.iter().filter(|k| !kept.contains(k)).cloned());
        }
        // Records not part of any device: browse PTRs for types, device-info, ...
        for (key, r) in &self.records {
            if !attributed.contains(key)
                && !r.is_live(now)
                && now.saturating_duration_since(r.last_seen) > self.timeout
            {
                remove.insert(key.clone());
            }
        }
        for key in remove {
            self.records.remove(&key);
            if let Some(set) = self.by_owner.get_mut(key.name.key()) {
                set.remove(&key);
                if set.is_empty() {
                    self.by_owner.remove(key.name.key());
                }
            }
        }
        self.refinements.retain(|_, r| {
            r.next_retry.is_some() || now.saturating_duration_since(r.requested) < REFINE_INTERVAL
        });
    }

    #[cfg(test)]
    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}

/// Channels to talk to a running store task.
pub struct StoreHandle {
    pub observations: mpsc::Sender<Observation>,
    pub retain: mpsc::UnboundedSender<Vec<String>>,
    pub snapshots: watch::Receiver<Arc<Snapshot>>,
}

/// Spawns the store task. Refinement queries go to `queries`, if any.
pub fn spawn(
    timeout: Duration,
    queries: Option<mpsc::UnboundedSender<QueryRequest>>,
) -> (StoreHandle, tokio::task::JoinHandle<()>) {
    let (obs_tx, obs_rx) = mpsc::channel(1024);
    let (retain_tx, retain_rx) = mpsc::unbounded_channel();
    let (snap_tx, snap_rx) = watch::channel(Arc::new(Snapshot::empty(Instant::now())));
    let task = tokio::spawn(run(Store::new(timeout), obs_rx, retain_rx, snap_tx, queries));
    (
        StoreHandle {
            observations: obs_tx,
            retain: retain_tx,
            snapshots: snap_rx,
        },
        task,
    )
}

async fn run(
    mut store: Store,
    mut observations: mpsc::Receiver<Observation>,
    mut retain: mpsc::UnboundedReceiver<Vec<String>>,
    snapshots: watch::Sender<Arc<Snapshot>>,
    queries: Option<mpsc::UnboundedSender<QueryRequest>>,
) {
    let mut publish = tokio::time::interval(PUBLISH_INTERVAL);
    publish.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut gc = tokio::time::interval(GC_INTERVAL);
    gc.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut dirty = true;
    let mut retain_open = true;

    loop {
        tokio::select! {
            obs = observations.recv() => {
                let Some(obs) = obs else { break };
                send_queries(&queries, store.ingest(obs));
                dirty = true;
            }
            ids = retain.recv(), if retain_open => {
                match ids {
                    Some(ids) => {
                        store.set_retained(ids);
                        store.gc(Instant::now());
                        // Publish right away so a released entry disappears without delay.
                        let _ = snapshots.send(Arc::new(store.snapshot(Instant::now())));
                        dirty = false;
                    }
                    None => retain_open = false,
                }
            }
            _ = publish.tick() => {
                if dirty {
                    let _ = snapshots.send(Arc::new(store.snapshot(Instant::now())));
                    dirty = false;
                }
            }
            _ = gc.tick() => {
                let now = Instant::now();
                store.gc(now);
                send_queries(&queries, store.due_refinements(now));
                dirty = true;
            }
        }
    }
}

fn send_queries(queries: &Option<mpsc::UnboundedSender<QueryRequest>>, requests: Vec<QueryRequest>) {
    if let Some(tx) = queries {
        for q in requests {
            let _ = tx.send(q);
        }
    }
}
