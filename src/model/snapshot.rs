//! Immutable view of the inventory, derived from the store's records.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Duration, SystemTime};

use tokio::time::Instant;

use super::record::{DnsName, RData, RecordKey, SourceState, Sources, rtype};
use super::service_names;
use super::store::StoredRecord;

/// Device id of the bucket holding instances whose host is not yet known.
pub const UNRESOLVED_ID: &str = "\u{0}unresolved";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Live,
    Stale,
    Left,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub system: SourceState,
    pub wire: SourceState,
    pub interfaces: Vec<String>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            system: SourceState::Starting,
            wire: SourceState::Starting,
            interfaces: Vec::new(),
        }
    }
}

impl Status {
    /// "avahi + wire", "wire only", "system only", ...
    pub fn sources_label(&self, system_name: &str) -> String {
        let system = self.system == SourceState::Active;
        let wire = self.wire == SourceState::Active;
        match (system, wire) {
            (true, true) => format!("{system_name} + wire"),
            (false, true) => "wire only".to_string(),
            (true, false) => "system only".to_string(),
            (false, false) => match (&self.system, &self.wire) {
                (SourceState::Starting, _) | (_, SourceState::Starting) => "starting".to_string(),
                _ => "no sources".to_string(),
            },
        }
    }

    pub fn system_only(&self) -> bool {
        matches!(self.wire, SourceState::Unavailable(_))
    }
}

/// One record as shown in the raw view.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordView {
    pub name: DnsName,
    pub data: RData,
    pub ttl: Option<u32>,
    pub remaining: Option<Duration>,
    pub cache_flush: bool,
    pub src_addr: Option<IpAddr>,
    pub iface: Option<String>,
    pub sources: Sources,
    pub first_seen: SystemTime,
    pub last_seen: SystemTime,
    pub expired: bool,
    pub goodbye: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    /// Instance name key.
    pub id: String,
    pub instance: DnsName,
    /// Instance label, e.g. "Office Printer".
    pub name: String,
    /// `_ipp._tcp`
    pub service_type: String,
    pub type_label: String,
    pub host: Option<DnsName>,
    pub device_id: String,
    pub port: Option<u16>,
    /// Decoded TXT entries sorted by key; value `None` for a bare key.
    pub txt: Vec<(String, Option<Vec<u8>>)>,
    pub addresses: Vec<IpAddr>,
    pub state: Liveness,
    pub last_seen: Instant,
    pub retained: bool,
    pub gone: bool,
    /// The service's own records plus its host's records.
    pub records: Vec<RecordView>,
    pub(crate) own_keys: Vec<RecordKey>,
}

/// One host name of a device, for per-host garbage collection.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Member {
    pub id: String,
    pub host_keys: Vec<RecordKey>,
    pub services: Vec<String>,
    /// This host on its own is past the device timeout.
    pub gone: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    /// Key of the primary host name, or [`UNRESOLVED_ID`].
    pub id: String,
    /// The primary host name.
    pub host: Option<DnsName>,
    /// All host names of the device, primary first.
    pub hosts: Vec<DnsName>,
    pub label: String,
    pub addresses: Vec<IpAddr>,
    /// Service ids sorted by display order.
    pub services: Vec<String>,
    pub state: Liveness,
    pub last_seen: Instant,
    pub retained: bool,
    pub gone: bool,
    /// Records owned by any of the device's host names.
    pub(crate) host_keys: Vec<RecordKey>,
    pub(crate) members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServiceType {
    /// `_ipp._tcp`
    pub id: String,
    pub label: String,
    /// Service ids sorted by display order.
    pub instances: Vec<String>,
    pub state: Liveness,
    pub retained: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub taken_at: Instant,
    pub devices: Vec<Device>,
    pub types: Vec<ServiceType>,
    pub services: HashMap<String, Service>,
    pub status: Status,
}

impl Snapshot {
    pub fn empty(now: Instant) -> Self {
        Self {
            taken_at: now,
            devices: Vec::new(),
            types: Vec::new(),
            services: HashMap::new(),
            status: Status::default(),
        }
    }

    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn service_type(&self, id: &str) -> Option<&ServiceType> {
        self.types.iter().find(|t| t.id == id)
    }
}

pub(crate) struct DeriveInput<'a> {
    pub records: &'a HashMap<RecordKey, StoredRecord>,
    pub by_owner: &'a HashMap<String, HashSet<RecordKey>>,
    pub now: Instant,
    pub timeout: Duration,
    pub retained: &'a HashSet<String>,
    pub status: &'a Status,
}

impl DeriveInput<'_> {
    fn owned(&self, name: &DnsName) -> impl Iterator<Item = (&RecordKey, &StoredRecord)> {
        self.by_owner
            .get(name.key())
            .into_iter()
            .flatten()
            .filter_map(|k| self.records.get_key_value(k))
    }

    /// Last-seen time of a record; daemon-held records count as seen now
    /// when the daemon is our only source.
    fn seen(&self, r: &StoredRecord) -> Instant {
        if self.status.system_only() && r.held_by_system {
            self.now
        } else {
            r.last_seen
        }
    }

    fn view(&self, r: &StoredRecord) -> RecordView {
        RecordView {
            name: r.name.clone(),
            data: r.data.clone(),
            ttl: r.ttl,
            remaining: r.expires.map(|e| e.saturating_duration_since(self.now)),
            cache_flush: r.cache_flush,
            src_addr: r.src_addr,
            iface: r.iface.clone(),
            sources: r.sources,
            first_seen: r.first_seen_wall,
            last_seen: r.last_seen_wall,
            expired: !r.is_live(self.now),
            goodbye: r.goodbye.is_some(),
        }
    }

    /// Picks the record to describe a name/type: live first, then most recent.
    fn best<'r>(&self, records: &[&'r StoredRecord], rt: u16) -> Option<&'r StoredRecord> {
        records
            .iter()
            .copied()
            .filter(|r| r.data.rtype() == rt)
            .max_by_key(|r| (r.is_live(self.now), r.last_seen))
    }

    fn liveness<'r>(&self, records: impl IntoIterator<Item = &'r StoredRecord>) -> Liveness {
        let mut any_goodbye = false;
        for r in records {
            if r.is_live(self.now) {
                return Liveness::Live;
            }
            any_goodbye |= r.goodbye.is_some();
        }
        if any_goodbye {
            Liveness::Left
        } else {
            Liveness::Stale
        }
    }

    fn last_seen<'r>(&self, records: impl IntoIterator<Item = &'r StoredRecord>) -> Instant {
        records
            .into_iter()
            .map(|r| self.seen(r))
            .max()
            .unwrap_or(self.now)
    }

    fn is_gone(&self, last_seen: Instant) -> bool {
        self.now.saturating_duration_since(last_seen) > self.timeout
    }
}

struct DeviceBuilder {
    host: Option<DnsName>,
    services: Vec<String>,
}

/// One host name with everything derived from it alone.
struct HostView<'a> {
    id: String,
    host: Option<DnsName>,
    services: Vec<String>,
    host_entries: Vec<(&'a RecordKey, &'a StoredRecord)>,
    label: String,
    /// Addresses used to decide merging: live ones, else the last known.
    merge_addresses: BTreeSet<IpAddr>,
    last_seen: Instant,
}

/// Minimal union-find over host indices.
struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(n: usize) -> Self {
        Self((0..n).collect())
    }

    fn find(&mut self, i: usize) -> usize {
        let parent = self.0[i];
        if parent == i {
            return i;
        }
        let root = self.find(parent);
        self.0[i] = root;
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[rb] = ra;
        }
    }
}

/// Groups host names that share a label (ignoring case) and an address.
fn group_hosts(hosts: &[HostView<'_>]) -> Vec<Vec<usize>> {
    let mut uf = UnionFind::new(hosts.len());
    let mut by_label: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, h) in hosts.iter().enumerate() {
        if h.id != UNRESOLVED_ID && !h.merge_addresses.is_empty() {
            by_label.entry(h.label.to_lowercase()).or_default().push(i);
        }
    }
    for bucket in by_label.values() {
        for (n, &a) in bucket.iter().enumerate() {
            for &b in &bucket[n + 1..] {
                if !hosts[a].merge_addresses.is_disjoint(&hosts[b].merge_addresses) {
                    uf.union(a, b);
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..hosts.len() {
        let root = uf.find(i);
        groups.entry(root).or_default().push(i);
    }
    groups.into_values().collect()
}

pub(crate) fn derive(input: &DeriveInput<'_>) -> Snapshot {
    // Service instances: browse PTR targets and SRV owners.
    let mut instances: BTreeMap<String, DnsName> = BTreeMap::new();
    let mut device_models: HashMap<String, String> = HashMap::new();
    for r in input.records.values() {
        match &r.data {
            RData::Ptr(target) if r.name.is_service_type() && target.is_service_instance() => {
                instances.insert(target.key().to_string(), target.clone());
            }
            RData::Srv { .. } if r.name.is_service_instance() => {
                instances.insert(r.name.key().to_string(), r.name.clone());
            }
            RData::Txt(strings)
                if r.name.instance_type().as_deref() == Some("_device-info._tcp") =>
            {
                if let Some(model) = txt_entries(strings)
                    .into_iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("model"))
                    .and_then(|(_, v)| v)
                {
                    device_models.insert(
                        r.name.first_label().to_lowercase(),
                        String::from_utf8_lossy(&model).into_owned(),
                    );
                }
            }
            _ => {}
        }
    }
    // Device-info pseudo-instances carry metadata, not a browsable service.
    instances.retain(|_, n| n.instance_type().as_deref() != Some("_device-info._tcp"));

    let mut devices: BTreeMap<String, DeviceBuilder> = BTreeMap::new();
    let mut services: HashMap<String, Service> = HashMap::new();

    for (id, instance) in &instances {
        let type_name = instance.parent();
        let mut own: Vec<(&RecordKey, &StoredRecord)> = input
            .owned(&type_name)
            .filter(|(_, r)| matches!(&r.data, RData::Ptr(t) if t == instance))
            .collect();
        own.extend(input.owned(instance));
        let own_records: Vec<&StoredRecord> = own.iter().map(|(_, r)| *r).collect();

        let srv = input.best(&own_records, rtype::SRV);
        let (host, port) = match srv.map(|r| &r.data) {
            Some(RData::Srv { target, port, .. }) => (Some(target.clone()), Some(*port)),
            _ => (None, None),
        };
        let txt = match input.best(&own_records, rtype::TXT).map(|r| &r.data) {
            Some(RData::Txt(strings)) => {
                let mut entries = txt_entries(strings);
                entries.sort_by_key(|e| e.0.to_lowercase());
                entries
            }
            _ => Vec::new(),
        };

        let host_records: Vec<&StoredRecord> = host
            .as_ref()
            .map(|h| input.owned(h).map(|(_, r)| r).collect())
            .unwrap_or_default();
        let addresses = addresses_of(&host_records);

        let mut records: Vec<RecordView> = own_records.iter().map(|r| input.view(r)).collect();
        records.sort_by_key(record_order);
        let mut host_views: Vec<RecordView> = host_records.iter().map(|r| input.view(r)).collect();
        host_views.sort_by_key(record_order);
        records.extend(host_views);

        let service_type = instance.instance_type().unwrap_or_default();
        let last_seen = input.last_seen(own_records.iter().copied());
        let device_id = host
            .as_ref()
            .map(|h| h.key().to_string())
            .unwrap_or_else(|| UNRESOLVED_ID.to_string());

        devices
            .entry(device_id.clone())
            .or_insert_with(|| DeviceBuilder {
                host: host.clone(),
                services: Vec::new(),
            })
            .services
            .push(id.clone());

        services.insert(
            id.clone(),
            Service {
                id: id.clone(),
                instance: instance.clone(),
                name: instance.first_label().to_string(),
                type_label: service_names::label(&service_type),
                service_type,
                host,
                device_id,
                port,
                txt,
                addresses,
                state: input.liveness(own_records.iter().copied()),
                last_seen,
                retained: input.retained.contains(id),
                gone: input.is_gone(last_seen),
                records,
                own_keys: own.iter().map(|(k, _)| (*k).clone()).collect(),
            },
        );
    }

    // Hosts that only have address records.
    for r in input.records.values() {
        if r.data.address().is_some() {
            devices
                .entry(r.name.key().to_string())
                .or_insert_with(|| DeviceBuilder {
                    host: Some(r.name.clone()),
                    services: Vec::new(),
                });
        }
    }

    let host_views: Vec<HostView> = devices
        .into_iter()
        .map(|(id, builder)| {
            let host_entries: Vec<(&RecordKey, &StoredRecord)> = builder
                .host
                .as_ref()
                .map(|h| input.owned(h).collect())
                .unwrap_or_default();
            let mut svc_ids = builder.services;
            sort_services(&mut svc_ids, &services);
            let records = host_entries.iter().map(|(_, r)| *r).chain(
                svc_ids
                    .iter()
                    .flat_map(|s| services[s].own_keys.iter())
                    .filter_map(|k| input.records.get(k)),
            );
            let last_seen = input.last_seen(records);
            let address_records = host_entries.iter().map(|(_, r)| *r).filter(|r| r.data.address().is_some());
            let live: BTreeSet<IpAddr> = address_records
                .clone()
                .filter(|r| r.is_live(input.now))
                .filter_map(|r| r.data.address())
                .collect();
            let merge_addresses = if live.is_empty() {
                address_records.filter_map(|r| r.data.address()).collect()
            } else {
                live
            };
            HostView {
                label: device_label(builder.host.as_ref(), &svc_ids, &services, &device_models),
                id,
                host: builder.host,
                services: svc_ids,
                host_entries,
                merge_addresses,
                last_seen,
            }
        })
        .collect();

    let mut device_list: Vec<Device> = Vec::new();
    for group in group_hosts(&host_views) {
        let mut members: Vec<&HostView> = group.iter().map(|&i| &host_views[i]).collect();
        // Primary first: most services, then the smallest key.
        members.sort_by_key(|h| (Reverse(h.services.len()), h.id.clone()));
        let primary = members[0];
        let id = primary.id.clone();

        let mut svc_ids: Vec<String> = members.iter().flat_map(|h| h.services.iter().cloned()).collect();
        sort_services(&mut svc_ids, &services);
        for sid in &svc_ids {
            if let Some(s) = services.get_mut(sid) {
                s.device_id = id.clone();
            }
        }
        let host_records: Vec<&StoredRecord> = members
            .iter()
            .flat_map(|h| h.host_entries.iter().map(|(_, r)| *r))
            .collect();
        let all_records = host_records.iter().copied().chain(
            svc_ids
                .iter()
                .flat_map(|s| services[s].own_keys.iter())
                .filter_map(|k| input.records.get(k)),
        );
        let state = input.liveness(all_records.clone());
        let last_seen = input.last_seen(all_records);

        let mut hosts: Vec<DnsName> = vec![];
        if let Some(h) = &primary.host {
            hosts.push(h.clone());
        }
        let mut others: Vec<DnsName> = members[1..].iter().filter_map(|h| h.host.clone()).collect();
        others.sort();
        hosts.extend(others);

        device_list.push(Device {
            retained: members.iter().any(|h| input.retained.contains(&h.id)),
            gone: input.is_gone(last_seen),
            addresses: addresses_of(&host_records),
            host_keys: members
                .iter()
                .flat_map(|h| h.host_entries.iter().map(|(k, _)| (*k).clone()))
                .collect(),
            members: members
                .iter()
                .map(|h| Member {
                    id: h.id.clone(),
                    host_keys: h.host_entries.iter().map(|(k, _)| (*k).clone()).collect(),
                    services: h.services.clone(),
                    gone: input.is_gone(h.last_seen),
                })
                .collect(),
            label: primary.label.clone(),
            host: primary.host.clone(),
            hosts,
            id,
            services: svc_ids,
            state,
            last_seen,
        });
    }
    device_list.sort_by(|a, b| {
        a.label
            .to_lowercase()
            .cmp(&b.label.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut by_type: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in services.values() {
        by_type
            .entry(s.service_type.clone())
            .or_default()
            .push(s.id.clone());
    }
    let mut types: Vec<ServiceType> = by_type
        .into_iter()
        .map(|(id, mut instances)| {
            sort_services(&mut instances, &services);
            let state = if instances
                .iter()
                .any(|i| services[i].state == Liveness::Live)
            {
                Liveness::Live
            } else if instances
                .iter()
                .any(|i| services[i].state == Liveness::Left)
            {
                Liveness::Left
            } else {
                Liveness::Stale
            };
            ServiceType {
                label: service_names::label(&id),
                retained: input.retained.contains(&id),
                id,
                instances,
                state,
            }
        })
        .collect();
    types.sort_by(|a, b| {
        a.label
            .to_lowercase()
            .cmp(&b.label.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });

    Snapshot {
        taken_at: input.now,
        devices: device_list,
        types,
        services,
        status: input.status.clone(),
    }
}

fn sort_services(ids: &mut [String], services: &HashMap<String, Service>) {
    ids.sort_by(|a, b| {
        let (a, b) = (&services[a], &services[b]);
        (a.name.to_lowercase(), a.type_label.to_lowercase(), &a.id).cmp(&(
            b.name.to_lowercase(),
            b.type_label.to_lowercase(),
            &b.id,
        ))
    });
}

fn device_label(
    host: Option<&DnsName>,
    service_ids: &[String],
    services: &HashMap<String, Service>,
    models: &HashMap<String, String>,
) -> String {
    let Some(host) = host else {
        return "(unresolved)".to_string();
    };
    let names = service_ids.iter().map(|s| services[s].name.as_str());
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for n in names.clone() {
        *counts.entry(n).or_default() += 1;
    }
    // Highest count wins; BTreeMap order makes ties resolve alphabetically.
    let mut best: Option<(&str, usize)> = None;
    for (name, count) in counts {
        if best.is_none_or(|(_, c)| count > c) {
            best = Some((name, count));
        }
    }
    if let Some((name, _)) = best {
        return name.to_string();
    }
    std::iter::once(host.first_label())
        .chain(names)
        .find_map(|n| models.get(&n.to_lowercase()))
        .cloned()
        .unwrap_or_else(|| host.to_string())
}

fn addresses_of(records: &[&StoredRecord]) -> Vec<IpAddr> {
    let mut addrs: Vec<IpAddr> = records.iter().filter_map(|r| r.data.address()).collect();
    addrs.sort();
    addrs.dedup();
    addrs
}

fn record_order(v: &RecordView) -> (u8, String) {
    let rank = match v.data.rtype() {
        rtype::PTR => 0,
        rtype::SRV => 1,
        rtype::TXT => 2,
        rtype::A => 3,
        rtype::AAAA => 4,
        _ => 5,
    };
    (rank, format!("{:?}", v.data))
}

/// Splits TXT strings into `key` / `key=value` entries. Empty strings are skipped.
pub fn txt_entries(strings: &[Vec<u8>]) -> Vec<(String, Option<Vec<u8>>)> {
    strings
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| match s.iter().position(|&b| b == b'=') {
            Some(i) => (
                String::from_utf8_lossy(&s[..i]).into_owned(),
                Some(s[i + 1..].to_vec()),
            ),
            None => (String::from_utf8_lossy(s).into_owned(), None),
        })
        .collect()
}
