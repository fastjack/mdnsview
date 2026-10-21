//! Limits a daemon backend's results to the interfaces in use.
//!
//! Daemons report each item on a *scope*, roughly an interface (avahi:
//! interface and protocol; mDNSResponder: interface index; Windows DNS: one
//! scope per instance). The [`ScopeBook`] keeps the latest copy of every
//! record per scope, passes on only records whose interface is in use,
//! releases instances that lose their last scope in use, and replays the
//! cached records of a scope when its interface is in use again.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::time::SystemTime;

use tokio::time::Instant;

use crate::model::record::{DnsName, Observation, ObservedRecord, RData};
use crate::wire::monitor::InterfaceSet;

/// Names of the interfaces in use. A record without an interface name
/// always counts as in use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InUse(Option<HashSet<String>>);

impl InUse {
    /// Every interface counts as in use.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub fn all() -> Self {
        Self(None)
    }

    pub fn of(set: &InterfaceSet) -> Self {
        Self(Some(set.active.iter().map(|i| i.name.clone()).collect()))
    }

    #[cfg(test)]
    pub fn names(names: &[&str]) -> Self {
        Self(Some(names.iter().map(|n| n.to_string()).collect()))
    }

    fn contains(&self, iface: Option<&str>) -> bool {
        match (&self.0, iface) {
            (_, None) | (None, _) => true,
            (Some(names), Some(name)) => names.contains(name),
        }
    }
}

/// The cache slot a record occupies: a newer copy replaces an older one.
/// SRV, TXT and PTR records keep one copy per owner and type (a re-resolve
/// replaces the old port or TXT); address records keep one copy per
/// address, so a host with several addresses keeps all of them.
type Slot = (String, u16, Option<RData>);

fn slot(obs: &Observation) -> Option<Slot> {
    let Observation::Record(r) = obs else {
        return None;
    };
    let address = r.data.address().map(|_| r.data.clone());
    let data = match &r.data {
        // Every instance of a type shares the PTR owner, so the target
        // tells them apart.
        RData::Ptr(_) => Some(r.data.clone()),
        _ => address,
    };
    Some((r.name.key().to_string(), r.data.rtype(), data))
}

#[derive(Debug)]
struct ScopeEntry {
    iface: Option<String>,
    records: Vec<(Slot, Observation)>,
}

impl ScopeEntry {
    fn new(iface: Option<String>) -> Self {
        Self { iface, records: Vec::new() }
    }

    fn in_use(&self, in_use: &InUse) -> bool {
        in_use.contains(self.iface.as_deref())
    }

    fn store(&mut self, observations: &[Observation]) {
        for obs in observations {
            let Some(slot) = slot(obs) else { continue };
            match self.records.iter_mut().find(|(s, _)| *s == slot) {
                Some(cached) => cached.1 = obs.clone(),
                None => self.records.push((slot, obs.clone())),
            }
        }
    }

    fn replay(&self) -> impl Iterator<Item = Observation> + '_ {
        self.records.iter().map(|(_, obs)| refreshed(obs))
    }
}

#[derive(Debug)]
struct Instance<S> {
    name: DnsName,
    scopes: HashMap<S, ScopeEntry>,
}

impl<S> Instance<S> {
    fn any_in_use(&self, in_use: &InUse) -> bool {
        self.scopes.values().any(|e| e.in_use(in_use))
    }
}

#[derive(Debug)]
pub struct ScopeBook<S> {
    instances: HashMap<String, Instance<S>>,
    /// Address records of hosts, by (host key, scope). They are not tied
    /// to one instance, so they are only filtered and replayed.
    hosts: HashMap<(String, S), ScopeEntry>,
}

impl<S> Default for ScopeBook<S> {
    fn default() -> Self {
        Self {
            instances: HashMap::new(),
            hosts: HashMap::new(),
        }
    }
}

impl<S: Copy + Eq + Hash> ScopeBook<S> {
    /// Records what the daemon reported about `instance` on `scope` and
    /// returns what to pass on. `iface` is the scope's interface name, if
    /// known; it may change, e.g. when it is only learned from a resolve.
    pub fn add(
        &mut self,
        instance: &DnsName,
        scope: S,
        iface: Option<String>,
        observations: Vec<Observation>,
        in_use: &InUse,
    ) -> Vec<Observation> {
        let entry = self
            .instances
            .entry(instance.key().to_string())
            .or_insert_with(|| Instance {
                name: instance.clone(),
                scopes: HashMap::new(),
            });
        let before = entry.any_in_use(in_use);
        let scope_entry = entry.scopes.entry(scope).or_insert_with(|| ScopeEntry::new(None));
        scope_entry.iface = iface;
        scope_entry.store(&observations);
        let passes = scope_entry.in_use(in_use);
        let after = entry.any_in_use(in_use);
        if before && !after {
            return vec![released(instance)];
        }
        if !before && after {
            // The instance becomes visible: send everything in use, not
            // only what just arrived.
            return entry
                .scopes
                .values()
                .filter(|e| e.in_use(in_use))
                .flat_map(ScopeEntry::replay)
                .collect();
        }
        if passes { observations } else { Vec::new() }
    }

    /// Records address records of `host` on `scope`; returns them if the
    /// scope is in use.
    pub fn host(
        &mut self,
        host: &DnsName,
        scope: S,
        iface: Option<String>,
        observations: Vec<Observation>,
        in_use: &InUse,
    ) -> Vec<Observation> {
        let entry = self
            .hosts
            .entry((host.key().to_string(), scope))
            .or_insert_with(|| ScopeEntry::new(None));
        entry.iface = iface;
        entry.store(&observations);
        if entry.in_use(in_use) { observations } else { Vec::new() }
    }

    /// The daemon no longer reports `instance` on `scope`. Reports the
    /// removal once no scope in use is left.
    pub fn remove(&mut self, instance: &DnsName, scope: S, in_use: &InUse) -> Vec<Observation> {
        let Some(entry) = self.instances.get_mut(instance.key()) else {
            return Vec::new();
        };
        let before = entry.any_in_use(in_use);
        entry.scopes.remove(&scope);
        let after = entry.any_in_use(in_use);
        if entry.scopes.is_empty() {
            self.instances.remove(instance.key());
        }
        if before && !after {
            vec![Observation::SystemRemoved {
                instance: instance.clone(),
                at: Instant::now(),
            }]
        } else {
            Vec::new()
        }
    }

    /// The interfaces in use changed from `old` to `new`: release instances
    /// that lost their last scope in use, and replay scopes in use again.
    pub fn set_in_use(&self, old: &InUse, new: &InUse) -> Vec<Observation> {
        let mut out = Vec::new();
        for entry in self.instances.values() {
            if entry.any_in_use(old) && !entry.any_in_use(new) {
                out.push(released(&entry.name));
                continue;
            }
            for scope in entry.scopes.values().filter(|e| e.in_use(new) && !e.in_use(old)) {
                out.extend(scope.replay());
            }
        }
        for entry in self.hosts.values().filter(|e| e.in_use(new) && !e.in_use(old)) {
            out.extend(entry.replay());
        }
        out
    }
}

fn released(instance: &DnsName) -> Observation {
    Observation::SystemReleased {
        instance: instance.clone(),
        at: Instant::now(),
    }
}

/// A cached observation, stamped as received now.
fn refreshed(obs: &Observation) -> Observation {
    match obs {
        Observation::Record(r) => Observation::Record(ObservedRecord {
            at: Instant::now(),
            wall: SystemTime::now(),
            ..r.clone()
        }),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;
    use crate::model::record::{Source, rtype};

    fn name(s: &str) -> DnsName {
        DnsName::parse(s)
    }

    fn printer() -> DnsName {
        name("Printer._ipp._tcp.local")
    }

    fn rec(owner: &str, data: RData, iface: &str) -> Observation {
        Observation::Record(ObservedRecord {
            name: name(owner),
            data,
            ttl: None,
            cache_flush: false,
            source: Source::System,
            src_addr: None,
            iface: Some(iface.into()),
            at: Instant::now(),
            wall: SystemTime::now(),
        })
    }

    fn srv(port: u16, iface: &str) -> Observation {
        rec(
            "Printer._ipp._tcp.local",
            RData::Srv { priority: 0, weight: 0, port, target: name("printer.local") },
            iface,
        )
    }

    fn report(iface: &str) -> Vec<Observation> {
        vec![
            rec("_ipp._tcp.local", RData::Ptr(printer()), iface),
            srv(631, iface),
            rec("Printer._ipp._tcp.local", RData::Txt(vec![b"ty=Laser".to_vec()]), iface),
        ]
    }

    fn a(last: u8, iface: &str) -> Observation {
        rec("printer.local", RData::A([192, 168, 1, last].into()), iface)
    }

    fn types(out: &[Observation]) -> Vec<u16> {
        out.iter()
            .map(|o| match o {
                Observation::Record(r) => r.data.rtype(),
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    const WLAN: u32 = 3;
    const ETH: u32 = 2;

    fn add(book: &mut ScopeBook<u32>, scope: u32, iface: &str, in_use: &InUse) -> Vec<Observation> {
        book.add(&printer(), scope, Some(iface.into()), report(iface), in_use)
    }

    #[test]
    fn excluded_interface_emits_nothing() {
        let mut book = ScopeBook::default();
        let in_use = InUse::names(&["wlan0"]);
        assert!(add(&mut book, 9, "veth1a2b", &in_use).is_empty());
        assert!(book.remove(&printer(), 9, &in_use).is_empty());
    }

    #[test]
    fn in_use_interface_emits_records() {
        let mut book = ScopeBook::default();
        assert_eq!(add(&mut book, WLAN, "wlan0", &InUse::names(&["wlan0"])).len(), 3);
    }

    #[test]
    fn unknown_interface_counts_as_in_use() {
        let mut book = ScopeBook::default();
        let out = book.add(&printer(), 0, None, report("x"), &InUse::names(&[]));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn deselecting_the_only_interface_releases() {
        let mut book = ScopeBook::default();
        let old = InUse::names(&["wlan0", "enp3s0"]);
        add(&mut book, WLAN, "wlan0", &old);
        let out = book.set_in_use(&old, &InUse::names(&["enp3s0"]));
        assert!(matches!(&out[..], [Observation::SystemReleased { instance, .. }] if *instance == printer()));
    }

    #[test]
    fn seen_on_two_interfaces_stays_when_one_is_deselected() {
        let mut book = ScopeBook::default();
        let old = InUse::names(&["wlan0", "enp3s0"]);
        add(&mut book, WLAN, "wlan0", &old);
        add(&mut book, ETH, "enp3s0", &old);
        assert!(book.set_in_use(&old, &InUse::names(&["enp3s0"])).is_empty());
    }

    #[test]
    fn reselecting_replays_cached_records() {
        let mut book = ScopeBook::default();
        let only_eth = InUse::names(&["enp3s0"]);
        assert!(add(&mut book, WLAN, "wlan0", &only_eth).is_empty());
        let out = book.set_in_use(&only_eth, &InUse::names(&["wlan0", "enp3s0"]));
        assert_eq!(types(&out), vec![rtype::PTR, rtype::SRV, rtype::TXT]);
    }

    #[test]
    fn replay_uses_the_latest_copy() {
        let mut book = ScopeBook::default();
        let only_eth = InUse::names(&["enp3s0"]);
        add(&mut book, WLAN, "wlan0", &only_eth);
        book.add(&printer(), WLAN, Some("wlan0".into()), vec![srv(8631, "wlan0")], &only_eth);
        let out = book.set_in_use(&only_eth, &InUse::names(&["wlan0"]));
        let ports: Vec<u16> = out
            .iter()
            .filter_map(|o| match o {
                Observation::Record(ObservedRecord { data: RData::Srv { port, .. }, .. }) => Some(*port),
                _ => None,
            })
            .collect();
        assert_eq!(ports, vec![8631]);
    }

    #[test]
    fn removal_reported_only_when_last_scope_in_use_goes() {
        let mut book = ScopeBook::default();
        let both = InUse::names(&["wlan0", "enp3s0"]);
        add(&mut book, WLAN, "wlan0", &both);
        add(&mut book, ETH, "enp3s0", &both);
        assert!(book.remove(&printer(), WLAN, &both).is_empty());
        let out = book.remove(&printer(), ETH, &both);
        assert!(matches!(&out[..], [Observation::SystemRemoved { .. }]));
    }

    #[test]
    fn removal_after_release_is_not_reported_again() {
        let mut book = ScopeBook::default();
        let old = InUse::names(&["wlan0"]);
        add(&mut book, WLAN, "wlan0", &old);
        let new = InUse::names(&["enp3s0"]);
        book.set_in_use(&old, &new);
        assert!(book.remove(&printer(), WLAN, &new).is_empty());
    }

    #[test]
    fn interface_learned_later_releases_or_shows() {
        // Windows DNS: the PTR arrives before the resolve names the interface.
        let in_use = InUse::names(&["WLAN"]);
        let mut book = ScopeBook::default();
        let ptr = vec![rec("_ipp._tcp.local", RData::Ptr(printer()), "x")];
        assert_eq!(book.add(&printer(), (), None, ptr.clone(), &in_use).len(), 1);
        let out = book.add(&printer(), (), Some("vEthernet (WSL)".into()), vec![srv(631, "vEthernet (WSL)")], &in_use);
        assert!(matches!(&out[..], [Observation::SystemReleased { .. }]));

        let mut book = ScopeBook::default();
        book.add(&printer(), (), Some("vEthernet (WSL)".into()), ptr, &in_use);
        let out = book.add(&printer(), (), Some("WLAN".into()), vec![srv(631, "WLAN")], &in_use);
        assert_eq!(types(&out), vec![rtype::PTR, rtype::SRV], "becoming visible sends the cached PTR too");
    }

    #[test]
    fn host_addresses_filtered_and_replayed() {
        let mut book: ScopeBook<u32> = ScopeBook::default();
        let only_eth = InUse::names(&["enp3s0"]);
        let host = name("printer.local");
        assert!(book.host(&host, WLAN, Some("wlan0".into()), vec![a(40, "wlan0")], &only_eth).is_empty());
        assert!(book.host(&host, WLAN, Some("wlan0".into()), vec![a(41, "wlan0")], &only_eth).is_empty());
        assert_eq!(book.host(&host, ETH, Some("enp3s0".into()), vec![a(40, "enp3s0")], &only_eth).len(), 1);
        let out = book.set_in_use(&only_eth, &InUse::names(&["enp3s0", "wlan0"]));
        let addrs: HashSet<IpAddr> = out
            .iter()
            .filter_map(|o| match o {
                Observation::Record(r) => r.data.address(),
                _ => None,
            })
            .collect();
        assert_eq!(addrs.len(), 2, "both cached addresses of wlan0 replayed: {out:?}");
    }

    #[test]
    fn all_counts_everything_in_use() {
        let mut book = ScopeBook::default();
        assert_eq!(add(&mut book, WLAN, "veth1a2b", &InUse::all()).len(), 3);
    }
}
