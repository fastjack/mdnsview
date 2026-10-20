//! The wire backend task: receives and decodes mDNS traffic, sends the
//! scheduler's queries and follows interface changes.

use std::collections::{BTreeMap, HashSet};
use std::future::pending;
use std::io;
use std::net::IpAddr;
use std::time::{Duration, SystemTime};

use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior, sleep_until};
use tracing::{debug, info, warn};

use super::codec::{self, WireRecord};
use super::interfaces::{Interface, Selector};
use super::scheduler::{Question, Scheduler};
use super::sys::{self, MdnsSocket, Received};
use crate::model::record::{
    LOCAL_NETWORK_BLOCKED, Observation, ObservedRecord, QueryRequest, Source, SourceState,
};

const INTERFACE_POLL: Duration = Duration::from_secs(5);
/// How long the wire source may hear no other host, after its first query,
/// before it reports itself silent.
const SILENCE: Duration = Duration::from_secs(15);
const QUESTIONS_PER_PACKET: usize = 20;
const KNOWN_ANSWERS_PER_PACKET: usize = 30;

pub struct WireBackend {
    v4: Option<MdnsSocket>,
    v6: Option<MdnsSocket>,
    selector: Selector,
    scheduler: Scheduler,
    /// Joined interfaces by index.
    joined: BTreeMap<u32, Interface>,
    /// Addresses of every interface on the host, to tell our own packets
    /// from other hosts'.
    own: HashSet<IpAddr>,
    first_send: Option<Instant>,
    last_foreign: Option<Instant>,
}

impl WireBackend {
    /// Opens the sockets. Fails only if no socket at all can be opened.
    pub fn open(selector: Selector, ipv6: bool, device_timeout: Duration) -> io::Result<Self> {
        let v4 = MdnsSocket::open_v4()
            .inspect_err(|e| warn!("IPv4 mDNS socket unavailable: {e}"));
        let v6 = if ipv6 {
            MdnsSocket::open_v6()
                .inspect_err(|e| warn!("IPv6 mDNS socket unavailable: {e}"))
                .ok()
        } else {
            None
        };
        let v4 = match (v4, &v6) {
            (Ok(s), _) => Some(s),
            (Err(_), Some(_)) => None,
            (Err(e), None) => return Err(e),
        };
        Ok(Self {
            v4,
            v6,
            selector,
            scheduler: Scheduler::new(Instant::now(), device_timeout),
            joined: BTreeMap::new(),
            own: HashSet::new(),
            first_send: None,
            last_foreign: None,
        })
    }

    fn sockets(&self) -> impl Iterator<Item = &MdnsSocket> {
        self.v4.iter().chain(self.v6.iter())
    }

    /// Joins and leaves multicast groups to match the current interfaces.
    /// Returns true when the set of interfaces changed.
    fn sync_interfaces(&mut self) -> bool {
        let all = match sys::list_interfaces() {
            Ok(all) => all,
            Err(e) => {
                warn!("cannot list interfaces: {e}");
                return false;
            }
        };
        self.own = all
            .iter()
            .flat_map(|i| i.v4.iter().map(|&a| IpAddr::V4(a)).chain(i.v6.iter().map(|&a| IpAddr::V6(a))))
            .collect();
        let wanted: BTreeMap<u32, Interface> = self
            .selector
            .select(&all)
            .into_iter()
            .map(|i| (i.index, i))
            .collect();
        if wanted == self.joined {
            return false;
        }
        for (index, old) in &self.joined {
            if wanted.get(index) != Some(old) {
                for s in self.sockets() {
                    // Leaving fails harmlessly when the interface is already gone.
                    let _ = s.leave(old);
                }
            }
        }
        for (index, new) in &wanted {
            if self.joined.get(index) != Some(new) {
                for s in self.sockets() {
                    if !s.is_v6() && new.v4.is_empty() {
                        continue;
                    }
                    match s.join(new) {
                        Ok(()) => info!("joined mDNS group on {} ({})", new.name, ipv(s)),
                        Err(e) => warn!("cannot join mDNS group on {} ({}): {e}", new.name, ipv(s)),
                    }
                }
            }
        }
        self.joined = wanted;
        true
    }

    fn interface_names(&self) -> Vec<String> {
        self.joined.values().map(|i| i.name.clone()).collect()
    }

    /// Sends the questions on every joined interface and returns the result
    /// of each send attempt.
    fn send_questions(&self, questions: &[Question], now: Instant) -> Vec<io::Result<()>> {
        let mut results = Vec::new();
        for chunk in questions.chunks(QUESTIONS_PER_PACKET) {
            let mut known = self.scheduler.known_answers(chunk, now);
            known.truncate(KNOWN_ANSWERS_PER_PACKET);
            let bytes = match codec::encode_query(chunk, &known) {
                Ok(b) => b,
                Err(e) => {
                    warn!("{e}");
                    continue;
                }
            };
            for iface in self.joined.values() {
                for s in self.sockets() {
                    let has_address = if s.is_v6() { !iface.v6.is_empty() } else { !iface.v4.is_empty() };
                    if !has_address {
                        continue;
                    }
                    let result = s.send_to_group(&bytes, iface);
                    if let Err(e) = &result {
                        debug!("send on {} ({}) failed: {e}", iface.name, ipv(s));
                    }
                    results.push(result);
                }
            }
            debug!("sent {} questions, {} known answers", chunk.len(), known.len());
        }
        results
    }

    /// Turns a received datagram into observations.
    fn handle_packet(&mut self, bytes: &[u8], rx: &Received) -> Vec<Observation> {
        let Some(iface) = self.joined.get(&rx.ifindex) else {
            return Vec::new(); // not one of our interfaces
        };
        if !self.own.contains(&rx.src) {
            self.last_foreign = Some(Instant::now());
        }
        let iface_name = iface.name.clone();
        let decoded = match codec::decode(bytes) {
            Ok(d) => d,
            Err(e) => {
                debug!("from {} on {iface_name}: {e}", rx.src);
                return Vec::new();
            }
        };
        if !decoded.is_response {
            return Vec::new();
        }
        let now = Instant::now();
        let wall = SystemTime::now();
        decoded
            .records
            .into_iter()
            .map(|r: WireRecord| {
                self.scheduler.on_record(&r, now);
                Observation::Record(ObservedRecord {
                    name: r.name,
                    data: r.data,
                    ttl: Some(r.ttl),
                    cache_flush: r.cache_flush,
                    source: Source::Wire,
                    src_addr: Some(rx.src),
                    iface: Some(iface_name.clone()),
                    at: now,
                    wall,
                })
            })
            .collect()
    }

    pub async fn run(
        mut self,
        observations: mpsc::Sender<Observation>,
        mut queries: mpsc::UnboundedReceiver<QueryRequest>,
    ) {
        let send = |obs: Observation| {
            let tx = observations.clone();
            async move { tx.send(obs).await.is_ok() }
        };
        self.sync_interfaces();
        if !send(Observation::Interfaces(self.interface_names())).await
            || !send(Observation::SourceStatus {
                source: Source::Wire,
                state: SourceState::Active,
            })
            .await
        {
            return;
        }

        let mut poll = tokio::time::interval(INTERFACE_POLL);
        poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
        poll.tick().await;
        let mut buf4 = vec![0u8; 9000];
        let mut buf6 = vec![0u8; 9000];
        let mut queries_open = true;
        let mut blocked = false;
        let mut silent = false;

        loop {
            let due = self.scheduler.next_due();
            let packets: Vec<Observation> = tokio::select! {
                _ = poll.tick() => {
                    if self.sync_interfaces() {
                        self.scheduler.restart_enumeration(Instant::now());
                        if !send(Observation::Interfaces(self.interface_names())).await {
                            return;
                        }
                    }
                    // Windows Firewall may drop all incoming mDNS to us.
                    let now_silent = silence(self.first_send, self.last_foreign, Instant::now());
                    if cfg!(windows) && now_silent != silent {
                        silent = now_silent;
                        if silent {
                            warn!("no mDNS from other hosts for {}s: incoming traffic may be blocked", SILENCE.as_secs());
                        } else {
                            info!("mDNS from other hosts arrives again");
                        }
                        if !send(Observation::WireSilent(silent)).await {
                            return;
                        }
                    }
                    continue;
                }
                rx = recv_opt(self.v4.as_ref(), &mut buf4) => match rx {
                    Ok(rx) => {
                        let bytes = buf4[..rx.len].to_vec();
                        self.handle_packet(&bytes, &rx)
                    }
                    Err(e) => { debug!("IPv4 receive failed: {e}"); continue; }
                },
                rx = recv_opt(self.v6.as_ref(), &mut buf6) => match rx {
                    Ok(rx) => {
                        let bytes = buf6[..rx.len].to_vec();
                        self.handle_packet(&bytes, &rx)
                    }
                    Err(e) => { debug!("IPv6 receive failed: {e}"); continue; }
                },
                q = queries.recv(), if queries_open => {
                    match q {
                        Some(q) => self.scheduler.on_request(&q, Instant::now()),
                        None => queries_open = false,
                    }
                    continue;
                }
                _ = sleep_or_pending(due) => {
                    let now = Instant::now();
                    let questions = self.scheduler.take_due(now);
                    let results = self.send_questions(&questions, now);
                    if self.first_send.is_none() && results.iter().any(Result::is_ok) {
                        self.first_send = Some(now);
                    }
                    if cfg!(target_os = "macos")
                        && let Some(now_blocked) = local_network_blocked(&results)
                        && now_blocked != blocked
                    {
                        blocked = now_blocked;
                        let state = if blocked {
                            warn!("multicast sends fail: {LOCAL_NETWORK_BLOCKED}");
                            SourceState::Unavailable(LOCAL_NETWORK_BLOCKED.into())
                        } else {
                            info!("multicast sends work again");
                            SourceState::Active
                        };
                        if !send(Observation::SourceStatus { source: Source::Wire, state }).await {
                            return;
                        }
                    }
                    continue;
                }
            };
            for obs in packets {
                if !send(obs).await {
                    return;
                }
            }
        }
    }
}

/// Whether the wire source counts as silent: queries have gone out for at
/// least [`SILENCE`], and no other host was heard in that time.
fn silence(first_send: Option<Instant>, last_foreign: Option<Instant>, now: Instant) -> bool {
    let Some(first_send) = first_send else {
        return false;
    };
    let since = last_foreign.map_or(first_send, |f| f.max(first_send));
    now.duration_since(since) >= SILENCE
}

/// Whether a round of sends shows that macOS's Local Network privacy setting
/// blocks us: every attempt failed with "no route to host". `None` when
/// nothing was sent.
fn local_network_blocked(results: &[io::Result<()>]) -> Option<bool> {
    if results.is_empty() {
        return None;
    }
    Some(results.iter().all(|r| {
        r.as_ref()
            .is_err_and(|e| e.raw_os_error() == Some(libc::EHOSTUNREACH))
    }))
}

fn ipv(s: &MdnsSocket) -> &'static str {
    if s.is_v6() { "IPv6" } else { "IPv4" }
}

async fn recv_opt(socket: Option<&MdnsSocket>, buf: &mut [u8]) -> io::Result<Received> {
    match socket {
        Some(s) => s.recv(buf).await,
        None => pending().await,
    }
}

async fn sleep_or_pending(due: Option<Instant>) {
    match due {
        Some(t) => sleep_until(t).await,
        None => pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::InterfaceConfig;

    #[test]
    fn silence_after_fifteen_seconds_without_other_hosts() {
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        // Nothing sent yet: never silent.
        assert!(!silence(None, None, at(60)));
        // Before 15 s, and after 15 s without traffic from other hosts.
        assert!(!silence(Some(t0), None, at(14)));
        assert!(silence(Some(t0), None, at(15)));
        // Traffic at 10 s: silent only 15 s after that.
        assert!(!silence(Some(t0), Some(at(10)), at(20)));
        assert!(silence(Some(t0), Some(at(10)), at(25)));
        // Traffic heard before the first send counts from the send.
        assert!(!silence(Some(at(5)), Some(at(1)), at(19)));
        assert!(silence(Some(at(5)), Some(at(1)), at(20)));
    }

    #[test]
    fn local_network_blocked_only_when_every_send_is_unreachable() {
        let unreachable = || Err(io::Error::from_raw_os_error(libc::EHOSTUNREACH));
        let other = || Err(io::Error::from_raw_os_error(libc::ENETDOWN));
        assert_eq!(local_network_blocked(&[unreachable(), unreachable()]), Some(true));
        assert_eq!(local_network_blocked(&[unreachable(), Ok(())]), Some(false));
        assert_eq!(local_network_blocked(&[unreachable(), other()]), Some(false));
        assert_eq!(local_network_blocked(&[Ok(()), Ok(())]), Some(false));
        assert_eq!(local_network_blocked(&[]), None);
    }

    /// Runs against the real network: `cargo test live_wire -- --ignored --nocapture`.
    /// Set MDNSVIEW_LIVE_SECS to change the duration (default 6).
    #[tokio::test]
    #[ignore = "uses the real network"]
    async fn live_wire() {
        let secs = std::env::var("MDNSVIEW_LIVE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6);
        let selector = Selector::new(&InterfaceConfig::default()).unwrap();
        let backend = WireBackend::open(selector, true, Duration::from_secs(300)).unwrap();
        let (tx, mut rx) = mpsc::channel(4096);
        let (_qtx, qrx) = mpsc::unbounded_channel();
        let task = tokio::spawn(backend.run(tx, qrx));
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut records = 0;
        while let Ok(Some(obs)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            match obs {
                Observation::Record(r) => {
                    records += 1;
                    println!(
                        "{:<6} {:<5} ttl={:<5} {:<15} {} {:?}",
                        r.iface.unwrap_or_default(),
                        crate::model::record::rtype::name(r.data.rtype()),
                        r.ttl.unwrap_or(0),
                        r.src_addr.map(|a| a.to_string()).unwrap_or_default(),
                        r.name,
                        r.data
                    );
                }
                other => println!("{other:?}"),
            }
        }
        task.abort();
        assert!(records > 0, "no mDNS records received");
    }
}
