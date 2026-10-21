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

use super::scopes::{InUse, ScopeBook};
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

#[derive(Default)]
struct State {
    browsers: HashSet<String>,
    book: ScopeBook<Scope>,
    in_use: InUse,
    /// (instance key, scope) → resolver object path.
    resolvers: HashMap<(String, Scope), OwnedObjectPath>,
    /// Object paths of our own browsers and resolvers.
    paths: HashSet<String>,
    interface_names: HashMap<i32, String>,
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
        in_use: InUse::of(&interfaces.borrow_and_update()),
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
                let in_use = InUse::of(&interfaces.borrow_and_update());
                let out = state.book.set_in_use(&state.in_use, &in_use);
                state.in_use = in_use;
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
            state.book.add(&instance, scope, iface_name, vec![ptr], &state.in_use)
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
            state.book.remove(&instance, scope, &state.in_use)
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
            let found = vec![
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
            let scope = (ifindex, proto);
            let mut out = state.book.add(&instance, scope, iface_name.clone(), found, &state.in_use);
            let address = match parsed {
                Ok(IpAddr::V4(a)) => Some(RData::A(a)),
                Ok(IpAddr::V6(a)) => Some(RData::Aaaa(a)),
                Err(_) => {
                    debug!("unparsable address {address:?} for {host}");
                    None
                }
            };
            if let Some(data) = address {
                let record = observe(host.clone(), data, iface_name.clone(), None);
                out.extend(state.book.host(&host, scope, iface_name, vec![record], &state.in_use));
            }
            out
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

    #[test]
    fn instance_names_keep_dots_in_the_instance_label() {
        let n = instance_name("Mr. Printer", "_ipp._tcp", "local");
        assert_eq!(n.labels(), ["Mr. Printer", "_ipp", "_tcp", "local"]);
        assert!(n.is_service_instance());
    }

    /// Runs against the local avahi-daemon:
    /// `cargo test live_avahi -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs a running avahi-daemon"]
    async fn live_avahi() {
        let (tx, mut rx) = mpsc::channel(4096);
        let (_sets, sets_rx) = watch::channel(crate::wire::monitor::Monitor::default_set());
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
