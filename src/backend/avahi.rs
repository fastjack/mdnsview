//! Avahi over D-Bus: service type browser → service browsers → resolvers.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::num::NonZeroUsize;
use std::time::SystemTime;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tracing::{debug, info, warn};
use zbus::message::Type as MessageType;
use zbus::{Connection, MatchRule, Message, MessageStream, OwnedObjectPath};

use super::{BoxFuture, SystemBackend};
use crate::model::record::{DnsName, Observation, ObservedRecord, RData, Source, SourceState};
use crate::wire::monitor::InterfaceSet;

const SERVICE: &str = "org.freedesktop.Avahi";
const SERVER: &str = "org.freedesktop.Avahi.Server";
const TYPE_BROWSER: &str = "org.freedesktop.Avahi.ServiceTypeBrowser";
const SERVICE_BROWSER: &str = "org.freedesktop.Avahi.ServiceBrowser";
const RESOLVER: &str = "org.freedesktop.Avahi.ServiceResolver";
const IF_UNSPEC: i32 = -1;
const PROTO_UNSPEC: i32 = -1;
const DOMAIN: &str = "local";
const SIGNAL_QUEUE: usize = 8192;

pub struct AvahiBackend;

impl SystemBackend for AvahiBackend {
    fn name(&self) -> &'static str {
        "avahi"
    }

    fn run(
        self: Box<Self>,
        observations: mpsc::Sender<Observation>,
        interfaces: watch::Receiver<InterfaceSet>,
    ) -> BoxFuture {
        Box::pin(async move {
            let result = run(&observations, interfaces).await;
            let state = match result {
                Ok(()) => SourceState::Unavailable("avahi went away".into()),
                Err(e) => {
                    warn!("avahi unavailable: {e:#}");
                    SourceState::Unavailable(format!("{e:#}"))
                }
            };
            let _ = observations
                .send(Observation::SourceStatus {
                    source: Source::System,
                    state,
                })
                .await;
        })
    }
}

/// (interface, protocol) pair avahi reports an item on.
type Scope = (i32, i32);

/// What avahi reported on one scope of an instance.
#[derive(Debug, Default)]
struct ScopeEntry {
    iface: Option<String>,
    /// The browse PTR from `ItemNew`.
    ptr: Option<Observation>,
    /// The SRV / TXT / address records of the latest `Found`.
    found: Vec<Observation>,
}

impl ScopeEntry {
    fn in_use(&self, active: &HashSet<String>) -> bool {
        self.iface.as_ref().is_none_or(|name| active.contains(name))
    }

    fn observations(&self) -> impl Iterator<Item = &Observation> {
        self.ptr.iter().chain(self.found.iter())
    }
}

/// Avahi's items per instance and scope, so results can be limited to the
/// interfaces in use and replayed when an interface is in use again.
/// A scope whose interface name is unknown always counts as in use.
#[derive(Debug, Default)]
struct ScopeBook {
    /// Instance key → (instance, scopes avahi currently reports it on).
    instances: HashMap<String, (DnsName, HashMap<Scope, ScopeEntry>)>,
}

impl ScopeBook {
    fn any_in_use(scopes: &HashMap<Scope, ScopeEntry>, active: &HashSet<String>) -> bool {
        scopes.values().any(|e| e.in_use(active))
    }

    fn entry(&mut self, instance: &DnsName, scope: Scope, iface: Option<String>) -> &mut ScopeEntry {
        let (_, scopes) = self
            .instances
            .entry(instance.key().to_string())
            .or_insert_with(|| (instance.clone(), HashMap::new()));
        let entry = scopes.entry(scope).or_default();
        entry.iface = iface;
        entry
    }

    /// Records an `ItemNew`; returns the PTR if its scope is in use.
    fn item_new(
        &mut self,
        instance: &DnsName,
        scope: Scope,
        iface: Option<String>,
        ptr: Observation,
        active: &HashSet<String>,
    ) -> Vec<Observation> {
        let entry = self.entry(instance, scope, iface);
        entry.ptr = Some(ptr.clone());
        if entry.in_use(active) { vec![ptr] } else { Vec::new() }
    }

    /// Records a `Found`; returns its records if the scope is in use.
    fn found(
        &mut self,
        instance: &DnsName,
        scope: Scope,
        iface: Option<String>,
        found: Vec<Observation>,
        active: &HashSet<String>,
    ) -> Vec<Observation> {
        let entry = self.entry(instance, scope, iface);
        entry.found = found.clone();
        if entry.in_use(active) { found } else { Vec::new() }
    }

    /// Records an `ItemRemove`; reports the removal once no scope in use is left.
    fn item_remove(&mut self, instance: &DnsName, scope: Scope, active: &HashSet<String>) -> Vec<Observation> {
        let Some((_, scopes)) = self.instances.get_mut(instance.key()) else {
            return Vec::new();
        };
        let before = Self::any_in_use(scopes, active);
        scopes.remove(&scope);
        let after = Self::any_in_use(scopes, active);
        if scopes.is_empty() {
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
    /// that lost their last scope in use, and replay scopes that are in use again.
    fn set_active(&self, old: &HashSet<String>, new: &HashSet<String>) -> Vec<Observation> {
        let mut out = Vec::new();
        for (instance, scopes) in self.instances.values() {
            if Self::any_in_use(scopes, old) && !Self::any_in_use(scopes, new) {
                out.push(Observation::SystemReleased {
                    instance: instance.clone(),
                    at: Instant::now(),
                });
                continue;
            }
            for entry in scopes.values().filter(|e| e.in_use(new) && !e.in_use(old)) {
                out.extend(entry.observations().map(refreshed));
            }
        }
        out
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

#[derive(Default)]
struct State {
    browsers: HashSet<String>,
    book: ScopeBook,
    /// Names of the interfaces in use.
    active: HashSet<String>,
    /// (instance key, scope) → resolver object path.
    resolvers: HashMap<(String, Scope), OwnedObjectPath>,
    /// Object paths of our own browsers and resolvers.
    paths: HashSet<String>,
    interface_names: HashMap<i32, String>,
}

fn active_names(set: &InterfaceSet) -> HashSet<String> {
    set.active.iter().map(|i| i.name.clone()).collect()
}

async fn run(
    observations: &mpsc::Sender<Observation>,
    mut interfaces: watch::Receiver<InterfaceSet>,
) -> Result<()> {
    let conn = Connection::system()
        .await
        .context("cannot connect to the system D-Bus")?;
    // Subscribe before creating any browser: avahi may emit signals for a
    // new object before its creator could subscribe to that path.
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(SERVICE)
        .build()?;
    let mut signals = MessageStream::for_match_rule(rule, &conn, NonZeroUsize::new(SIGNAL_QUEUE))
        .await
        .context("cannot subscribe to avahi signals")?;
    let version: String = call(&conn, "GetVersionString", &())
        .await
        .context("avahi-daemon is not reachable")?;
    info!("connected to {version}");

    let mut state = State {
        active: active_names(&interfaces.borrow_and_update()),
        ..State::default()
    };
    let type_browser: OwnedObjectPath = call(
        &conn,
        "ServiceTypeBrowserNew",
        &(IF_UNSPEC, PROTO_UNSPEC, DOMAIN, 0u32),
    )
    .await?;
    state.paths.insert(type_browser.to_string());

    if observations
        .send(Observation::SourceStatus {
            source: Source::System,
            state: SourceState::Active,
        })
        .await
        .is_err()
    {
        return Ok(());
    }

    let mut interfaces_open = true;
    loop {
        let out = tokio::select! {
            msg = signals.next() => {
                let msg = match msg {
                    None => return Ok(()),
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        debug!("bad avahi signal: {e}");
                        continue;
                    }
                };
                if is_daemon_gone(&msg) {
                    return Ok(());
                }
                handle_signal(&conn, &mut state, &msg).await
            }
            changed = interfaces.changed(), if interfaces_open => {
                if changed.is_err() {
                    interfaces_open = false;
                    continue;
                }
                let active = active_names(&interfaces.borrow_and_update());
                let out = state.book.set_active(&state.active, &active);
                state.active = active;
                out
            }
        };
        for obs in out {
            if observations.send(obs).await.is_err() {
                return Ok(());
            }
        }
    }
}

async fn call<B, R>(conn: &Connection, method: &str, body: &B) -> Result<R>
where
    B: serde::Serialize + zbus::wire::DynamicType,
    R: for<'d> zbus::wire::DynamicDeserialize<'d>,
{
    let reply = conn
        .call_method(Some(SERVICE), "/", Some(SERVER), method, body)
        .await
        .with_context(|| format!("avahi {method} failed"))?;
    Ok(reply.body().deserialize::<R>()?)
}

fn is_daemon_gone(msg: &Message) -> bool {
    let header = msg.header();
    header.member().is_some_and(|m| m.as_str() == "StateChanged")
        && msg
            .body()
            .deserialize::<(i32, String)>()
            .is_ok_and(|(state, _)| state == 4) // AVAHI_SERVER_FAILURE
}

async fn handle_signal(conn: &Connection, state: &mut State, msg: &Message) -> Vec<Observation> {
    let header = msg.header();
    let (Some(iface), Some(member), Some(path)) = (header.interface(), header.member(), header.path())
    else {
        return Vec::new();
    };
    if !state.paths.contains(path.as_str()) {
        return Vec::new(); // another client's browser
    }
    let body = msg.body();
    match (iface.as_str(), member.as_str()) {
        (TYPE_BROWSER, "ItemNew") => {
            let Ok((_, _, service_type, domain, _)) =
                body.deserialize::<(i32, i32, String, String, u32)>()
            else {
                return Vec::new();
            };
            if state.browsers.insert(service_type.clone()) {
                match call::<_, OwnedObjectPath>(
                    conn,
                    "ServiceBrowserNew",
                    &(IF_UNSPEC, PROTO_UNSPEC, service_type.as_str(), domain.as_str(), 0u32),
                )
                .await
                {
                    Ok(p) => {
                        state.paths.insert(p.to_string());
                    }
                    Err(e) => warn!("cannot browse {service_type}: {e:#}"),
                }
            }
            let meta = DnsName::parse(&format!("_services._dns-sd._udp.{domain}"));
            let type_name = DnsName::parse(&format!("{service_type}.{domain}"));
            vec![observe(meta, RData::Ptr(type_name), None, None)]
        }
        (SERVICE_BROWSER, "ItemNew") => {
            let Ok((ifindex, proto, name, service_type, domain, _)) =
                body.deserialize::<(i32, i32, String, String, String, u32)>()
            else {
                return Vec::new();
            };
            let instance = instance_name(&name, &service_type, &domain);
            let scope = (ifindex, proto);
            let key = (instance.key().to_string(), scope);
            if !state.resolvers.contains_key(&key) {
                match call::<_, OwnedObjectPath>(
                    conn,
                    "ServiceResolverNew",
                    &(
                        ifindex,
                        proto,
                        name.as_str(),
                        service_type.as_str(),
                        domain.as_str(),
                        PROTO_UNSPEC,
                        0u32,
                    ),
                )
                .await
                {
                    Ok(p) => {
                        state.paths.insert(p.to_string());
                        state.resolvers.insert(key, p);
                    }
                    Err(e) => debug!("cannot resolve {instance}: {e:#}"),
                }
            }
            let iface_name = interface_name(conn, state, ifindex).await;
            let ptr = observe(instance.parent(), RData::Ptr(instance.clone()), iface_name.clone(), None);
            state.book.item_new(&instance, scope, iface_name, ptr, &state.active)
        }
        (SERVICE_BROWSER, "ItemRemove") => {
            let Ok((ifindex, proto, name, service_type, domain, _)) =
                body.deserialize::<(i32, i32, String, String, String, u32)>()
            else {
                return Vec::new();
            };
            let instance = instance_name(&name, &service_type, &domain);
            let scope = (ifindex, proto);
            if let Some(path) = state
                .resolvers
                .remove(&(instance.key().to_string(), scope))
            {
                state.paths.remove(path.as_str());
                let _ = conn
                    .call_method(Some(SERVICE), path.as_str(), Some(RESOLVER), "Free", &())
                    .await;
            }
            state.book.item_remove(&instance, scope, &state.active)
        }
        (RESOLVER, "Found") => {
            type Found = (
                i32,
                i32,
                String,
                String,
                String,
                String,
                i32,
                String,
                u16,
                Vec<Vec<u8>>,
                u32,
            );
            let Ok((ifindex, proto, name, service_type, domain, host, _, address, port, txt, _)) =
                body.deserialize::<Found>()
            else {
                return Vec::new();
            };
            let iface_name = interface_name(conn, state, ifindex).await;
            let instance = instance_name(&name, &service_type, &domain);
            let host = DnsName::parse(&host);
            let parsed = address.parse::<IpAddr>();
            // The resolved address stands in for the sender, so the TXT of
            // an instance name shared by several hosts can be told apart.
            let src = parsed.as_ref().ok().copied();
            let mut found = vec![
                observe(
                    instance.clone(),
                    RData::Srv {
                        priority: 0,
                        weight: 0,
                        port,
                        target: host.clone(),
                    },
                    iface_name.clone(),
                    src,
                ),
                observe(instance.clone(), RData::Txt(txt), iface_name.clone(), src),
            ];
            match parsed {
                Ok(IpAddr::V4(a)) => found.push(observe(host, RData::A(a), iface_name.clone(), None)),
                Ok(IpAddr::V6(a)) => found.push(observe(host, RData::Aaaa(a), iface_name.clone(), None)),
                Err(_) => debug!("unparsable address {address:?} for {host}"),
            }
            state.book.found(&instance, (ifindex, proto), iface_name, found, &state.active)
        }
        (_, "Failure") => {
            if let Ok((error,)) = body.deserialize::<(String,)>() {
                debug!("avahi {iface} {path} failed: {error}");
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn instance_name(name: &str, service_type: &str, domain: &str) -> DnsName {
    let mut labels = vec![name.to_string()];
    labels.extend(DnsName::parse(service_type).labels().iter().cloned());
    labels.extend(DnsName::parse(domain).labels().iter().cloned());
    DnsName::from_labels(labels)
}

async fn interface_name(conn: &Connection, state: &mut State, index: i32) -> Option<String> {
    if index < 0 {
        return None;
    }
    if let Some(name) = state.interface_names.get(&index) {
        return Some(name.clone());
    }
    let name: String = call(conn, "GetNetworkInterfaceNameByIndex", &(index,))
        .await
        .ok()?;
    state.interface_names.insert(index, name.clone());
    Some(name)
}

fn observe(name: DnsName, data: RData, iface: Option<String>, src_addr: Option<IpAddr>) -> Observation {
    Observation::Record(ObservedRecord {
        name,
        data,
        ttl: None,
        cache_flush: false,
        source: Source::System,
        src_addr,
        iface,
        at: Instant::now(),
        wall: SystemTime::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::record::rtype;

    fn active(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn printer() -> DnsName {
        instance_name("Printer", "_ipp._tcp", "local")
    }

    fn ptr_obs(iface: &str) -> Observation {
        observe(printer().parent(), RData::Ptr(printer()), Some(iface.into()), None)
    }

    fn found_obs(iface: &str) -> Vec<Observation> {
        let host = DnsName::parse("printer.local");
        vec![
            observe(
                printer(),
                RData::Srv { priority: 0, weight: 0, port: 631, target: host.clone() },
                Some(iface.into()),
                None,
            ),
            observe(printer(), RData::Txt(vec![b"ty=Laser".to_vec()]), Some(iface.into()), None),
            observe(host, RData::A("192.168.1.40".parse().unwrap()), Some(iface.into()), None),
        ]
    }

    const WLAN: Scope = (3, 0);
    const ETH: Scope = (2, 0);

    /// Reports the printer on `scope` and returns what was emitted.
    fn report(book: &mut ScopeBook, scope: Scope, iface: &str, active: &HashSet<String>) -> Vec<Observation> {
        let mut out = book.item_new(&printer(), scope, Some(iface.into()), ptr_obs(iface), active);
        out.extend(book.found(&printer(), scope, Some(iface.into()), found_obs(iface), active));
        out
    }

    #[test]
    fn excluded_interface_emits_nothing() {
        let mut book = ScopeBook::default();
        let in_use = active(&["wlan0"]);
        assert!(report(&mut book, (9, 0), "veth1a2b", &in_use).is_empty());
        assert!(book.item_remove(&printer(), (9, 0), &in_use).is_empty());
    }

    #[test]
    fn in_use_interface_emits_records() {
        let mut book = ScopeBook::default();
        assert_eq!(report(&mut book, WLAN, "wlan0", &active(&["wlan0"])).len(), 4);
    }

    #[test]
    fn unknown_interface_counts_as_in_use() {
        let mut book = ScopeBook::default();
        let out = book.item_new(&printer(), (-1, -1), None, ptr_obs("x"), &active(&[]));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn deselecting_the_only_interface_releases() {
        let mut book = ScopeBook::default();
        let old = active(&["wlan0", "enp3s0"]);
        report(&mut book, WLAN, "wlan0", &old);
        let out = book.set_active(&old, &active(&["enp3s0"]));
        assert!(matches!(&out[..], [Observation::SystemReleased { instance, .. }] if *instance == printer()));
    }

    #[test]
    fn seen_on_two_interfaces_stays_when_one_is_deselected() {
        let mut book = ScopeBook::default();
        let old = active(&["wlan0", "enp3s0"]);
        report(&mut book, WLAN, "wlan0", &old);
        report(&mut book, ETH, "enp3s0", &old);
        assert!(book.set_active(&old, &active(&["enp3s0"])).is_empty());
    }

    #[test]
    fn reselecting_replays_cached_records() {
        let mut book = ScopeBook::default();
        let only_eth = active(&["enp3s0"]);
        assert!(report(&mut book, WLAN, "wlan0", &only_eth).is_empty());
        let out = book.set_active(&only_eth, &active(&["wlan0", "enp3s0"]));
        let types: Vec<u16> = out
            .iter()
            .map(|o| match o {
                Observation::Record(r) => r.data.rtype(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(types, vec![rtype::PTR, rtype::SRV, rtype::TXT, rtype::A]);
    }

    #[test]
    fn removal_reported_only_when_last_scope_in_use_goes() {
        let mut book = ScopeBook::default();
        let both = active(&["wlan0", "enp3s0"]);
        report(&mut book, WLAN, "wlan0", &both);
        report(&mut book, ETH, "enp3s0", &both);
        assert!(book.item_remove(&printer(), WLAN, &both).is_empty());
        let out = book.item_remove(&printer(), ETH, &both);
        assert!(matches!(&out[..], [Observation::SystemRemoved { .. }]));
    }

    #[test]
    fn removal_after_release_is_not_reported_again() {
        let mut book = ScopeBook::default();
        let old = active(&["wlan0"]);
        report(&mut book, WLAN, "wlan0", &old);
        let new = active(&["enp3s0"]);
        book.set_active(&old, &new);
        assert!(book.item_remove(&printer(), WLAN, &new).is_empty());
    }

    #[test]
    fn instance_names_keep_dots_in_the_instance_label() {
        let n = instance_name("Mr. Printer", "_ipp._tcp", "local");
        assert_eq!(n.labels(), ["Mr. Printer", "_ipp", "_tcp", "local"]);
        assert!(n.is_service_instance());
    }

    fn live_interfaces() -> InterfaceSet {
        let eligible = crate::wire::interfaces::Selector::new(&Default::default())
            .unwrap()
            .select(&crate::wire::sys::list_interfaces().unwrap());
        InterfaceSet { active: eligible.clone(), eligible }
    }

    /// Runs against the local avahi-daemon:
    /// `cargo test live_avahi -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs a running avahi-daemon"]
    async fn live_avahi() {
        let (tx, mut rx) = mpsc::channel(4096);
        // Every eligible interface in use, as with the default configuration.
        let (_sets, sets_rx) = watch::channel(live_interfaces());
        let task = tokio::spawn(Box::new(AvahiBackend).run(tx, sets_rx));
        let deadline = Instant::now() + std::time::Duration::from_secs(4);
        let mut records = 0;
        while let Ok(Some(obs)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            match &obs {
                Observation::Record(r) => {
                    records += 1;
                    println!("{:<8} {} {:?}", r.iface.clone().unwrap_or_default(), r.name, r.data);
                }
                other => println!("{other:?}"),
            }
        }
        task.abort();
        assert!(records > 0);
    }
}
