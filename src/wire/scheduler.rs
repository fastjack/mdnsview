//! Decides which questions to send when: enumeration, refinement and
//! TTL-driven refresh (RFC 6762 §5.2).

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use tokio::time::Instant;

use super::codec::WireRecord;
use crate::model::record::{DnsName, QueryRequest, RData, rtype};

/// First enumeration interval; doubles up to [`ENUM_MAX_INTERVAL`].
const ENUM_FIRST_INTERVAL: Duration = Duration::from_secs(1);
const ENUM_MAX_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Fractions of the TTL at which refresh queries are sent.
const REFRESH_POINTS: [f64; 4] = [0.80, 0.85, 0.90, 0.95];
/// Maximum random jitter added to each refresh point, as a fraction of the TTL.
const REFRESH_JITTER: f64 = 0.02;
/// Questions falling due within this window are sent together.
const BATCH_WINDOW: Duration = Duration::from_millis(20);
/// Repeated refinement requests for the same name are ignored this long.
const REFINE_DEDUP: Duration = Duration::from_secs(1);

pub type Question = (DnsName, u16);

struct Enumeration {
    next: Instant,
    interval: Duration,
}

impl Enumeration {
    fn new(now: Instant) -> Self {
        Self {
            next: now,
            interval: ENUM_FIRST_INTERVAL,
        }
    }

    fn fired(&mut self) {
        self.next += self.interval;
        self.interval = (self.interval * 2).min(ENUM_MAX_INTERVAL);
    }
}

/// Cached PTR answer for known-answer suppression.
struct KnownPtr {
    ttl: u32,
    received: Instant,
}

pub struct Scheduler {
    meta: Enumeration,
    types: HashMap<DnsName, Enumeration>,
    /// Pending refresh times per record.
    refresh: HashMap<(DnsName, RData), Vec<Instant>>,
    /// Questions requested for immediate sending.
    immediate: BTreeSet<Question>,
    refine_seen: HashMap<DnsName, Instant>,
    known_ptrs: HashMap<(DnsName, DnsName), KnownPtr>,
    /// Upper bound for the refresh window of liveness records, so silent
    /// but healthy hosts are probed well within the device timeout.
    liveness_window: Duration,
    jitter: fn() -> f64,
}

fn random_jitter() -> f64 {
    rand::random_range(0.0..REFRESH_JITTER)
}

impl Scheduler {
    pub fn new(now: Instant, device_timeout: Duration) -> Self {
        Self::with_jitter(now, device_timeout, random_jitter)
    }

    pub fn with_jitter(now: Instant, device_timeout: Duration, jitter: fn() -> f64) -> Self {
        Self {
            meta: Enumeration::new(now),
            types: HashMap::new(),
            refresh: HashMap::new(),
            immediate: BTreeSet::new(),
            refine_seen: HashMap::new(),
            known_ptrs: HashMap::new(),
            liveness_window: device_timeout / 2,
            jitter,
        }
    }

    fn meta_name() -> DnsName {
        DnsName::parse("_services._dns-sd._udp.local")
    }

    /// Updates schedules for a record received on the wire.
    pub fn on_record(&mut self, rec: &WireRecord, now: Instant) {
        let id = (rec.name.clone(), rec.data.clone());
        if let RData::Ptr(target) = &rec.data {
            let key = (rec.name.clone(), target.clone());
            if rec.ttl == 0 {
                self.known_ptrs.remove(&key);
            } else {
                self.known_ptrs.insert(
                    key,
                    KnownPtr {
                        ttl: rec.ttl,
                        received: now,
                    },
                );
            }
            if rec.name.is_meta_query() && target.is_service_type() {
                self.types
                    .entry(target.clone())
                    .or_insert_with(|| Enumeration::new(now));
            }
        }
        if rec.ttl == 0 {
            self.refresh.remove(&id);
            return;
        }
        let rt = rec.data.rtype();
        if !matches!(rt, rtype::PTR | rtype::SRV | rtype::TXT | rtype::A | rtype::AAAA) {
            return;
        }
        let mut window = Duration::from_secs(rec.ttl.into());
        if matches!(rt, rtype::SRV | rtype::A | rtype::AAAA) {
            window = window.min(self.liveness_window);
        }
        let times = REFRESH_POINTS
            .iter()
            .map(|p| now + window.mul_f64(p + (self.jitter)()))
            .collect();
        self.refresh.insert(id, times);
    }

    /// Queues an immediate query for names the store wants refined.
    pub fn on_request(&mut self, req: &QueryRequest, now: Instant) {
        if let Some(last) = self.refine_seen.get(&req.name)
            && now.saturating_duration_since(*last) < REFINE_DEDUP
        {
            return;
        }
        self.refine_seen.insert(req.name.clone(), now);
        for rt in &req.rtypes {
            self.immediate.insert((req.name.clone(), *rt));
        }
    }

    /// Restarts enumeration, e.g. after an interface came up.
    pub fn restart_enumeration(&mut self, now: Instant) {
        self.meta = Enumeration::new(now);
        for e in self.types.values_mut() {
            *e = Enumeration::new(now);
        }
    }

    pub fn next_due(&self) -> Option<Instant> {
        if !self.immediate.is_empty() {
            return Some(Instant::now().min(self.meta.next));
        }
        std::iter::once(self.meta.next)
            .chain(self.types.values().map(|e| e.next))
            .chain(self.refresh.values().filter_map(|t| t.first().copied()))
            .min()
    }

    /// Removes and returns all questions due by `now` (plus the batching window).
    pub fn take_due(&mut self, now: Instant) -> Vec<Question> {
        let horizon = now + BATCH_WINDOW;
        let mut due: BTreeSet<Question> = std::mem::take(&mut self.immediate);
        if self.meta.next <= horizon {
            due.insert((Self::meta_name(), rtype::PTR));
            self.meta.fired();
        }
        for (name, e) in &mut self.types {
            if e.next <= horizon {
                due.insert((name.clone(), rtype::PTR));
                e.fired();
            }
        }
        for ((name, data), times) in &mut self.refresh {
            if times.first().is_some_and(|t| *t <= horizon) {
                while times.first().is_some_and(|t| *t <= horizon) {
                    times.remove(0);
                }
                due.insert((name.clone(), data.rtype()));
            }
        }
        self.refresh.retain(|_, times| !times.is_empty());
        self.refine_seen
            .retain(|_, at| now.saturating_duration_since(*at) < REFINE_DEDUP);
        due.into_iter().collect()
    }

    /// PTR answers for the given questions that still have more than half
    /// their TTL left (RFC 6762 §7.1).
    pub fn known_answers(&self, questions: &[Question], now: Instant) -> Vec<WireRecord> {
        let mut out = Vec::new();
        for ((owner, target), known) in &self.known_ptrs {
            if !questions.iter().any(|(q, t)| *t == rtype::PTR && q == owner) {
                continue;
            }
            let age = now.saturating_duration_since(known.received).as_secs();
            let total = u64::from(known.ttl);
            if age * 2 < total {
                out.push(WireRecord {
                    name: owner.clone(),
                    data: RData::Ptr(target.clone()),
                    ttl: (total - age) as u32,
                    cache_flush: false,
                });
            }
        }
        out.sort_by(|a, b| (&a.name, &a.data).cmp(&(&b.name, &b.data)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::advance;

    const TIMEOUT: Duration = Duration::from_secs(300);

    fn sched() -> Scheduler {
        Scheduler::with_jitter(Instant::now(), TIMEOUT, || 0.0)
    }

    fn rec(owner: &str, data: RData, ttl: u32) -> WireRecord {
        WireRecord {
            name: DnsName::parse(owner),
            data,
            ttl,
            cache_flush: false,
        }
    }

    fn a_rec(ttl: u32) -> WireRecord {
        rec("p.local", RData::A([10, 0, 0, 1].into()), ttl)
    }

    fn q(name: &str, rt: u16) -> Question {
        (DnsName::parse(name), rt)
    }

    /// Advances virtual time to each due point and collects (offset, questions).
    async fn run_for(s: &mut Scheduler, total: Duration) -> Vec<(Duration, Vec<Question>)> {
        let start = Instant::now();
        let mut out = Vec::new();
        while let Some(due) = s.next_due() {
            if due > start + total {
                break;
            }
            let now = Instant::now();
            if due > now {
                advance(due - now).await;
            }
            let qs = s.take_due(Instant::now());
            out.push((Instant::now() - start, qs));
        }
        out
    }

    #[tokio::test(start_paused = true)]
    async fn enumeration_backs_off_exponentially() {
        let mut s = sched();
        let fired = run_for(&mut s, Duration::from_secs(16)).await;
        let times: Vec<u64> = fired.iter().map(|(t, _)| t.as_secs()).collect();
        assert_eq!(times, vec![0, 1, 3, 7, 15]);
        assert!(fired.iter().all(|(_, qs)| qs == &vec![q("_services._dns-sd._udp.local", rtype::PTR)]));
    }

    #[tokio::test(start_paused = true)]
    async fn enumeration_interval_caps_at_one_hour() {
        let mut s = sched();
        for _ in 0..20 {
            s.meta.fired();
        }
        assert_eq!(s.meta.interval, ENUM_MAX_INTERVAL);
    }

    #[tokio::test(start_paused = true)]
    async fn discovered_types_are_enumerated() {
        let mut s = sched();
        s.take_due(Instant::now());
        s.on_record(
            &rec("_services._dns-sd._udp.local", RData::Ptr(DnsName::parse("_ipp._tcp.local")), 4500),
            Instant::now(),
        );
        let qs = s.take_due(Instant::now());
        assert_eq!(qs, vec![q("_ipp._tcp.local", rtype::PTR)]);
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_at_80_85_90_95_percent() {
        let mut s = sched();
        s.take_due(Instant::now());
        s.meta.next = Instant::now() + Duration::from_secs(10_000);
        s.on_record(&a_rec(120), Instant::now());
        let fired = run_for(&mut s, Duration::from_secs(120)).await;
        let times: Vec<u64> = fired.iter().map(|(t, _)| t.as_secs()).collect();
        assert_eq!(times, vec![96, 102, 108, 114]);
        assert!(fired.iter().all(|(_, qs)| qs == &vec![q("p.local", rtype::A)]));
    }

    #[tokio::test(start_paused = true)]
    async fn renewal_cancels_pending_refreshes() {
        let mut s = sched();
        s.take_due(Instant::now());
        s.meta.next = Instant::now() + Duration::from_secs(10_000);
        s.on_record(&a_rec(120), Instant::now());
        advance(Duration::from_secs(96)).await;
        assert_eq!(s.take_due(Instant::now()), vec![q("p.local", rtype::A)]);
        // The host answers: schedule restarts from now.
        s.on_record(&a_rec(120), Instant::now());
        let fired = run_for(&mut s, Duration::from_secs(100)).await;
        let times: Vec<u64> = fired.iter().map(|(t, _)| t.as_secs()).collect();
        // Only the 80% point of the new cycle; the old 85% point is gone.
        assert_eq!(times, vec![96]);
    }

    #[tokio::test(start_paused = true)]
    async fn long_ttl_addresses_probed_within_device_timeout() {
        let mut s = sched();
        s.take_due(Instant::now());
        s.meta.next = Instant::now() + Duration::from_secs(100_000);
        s.on_record(&a_rec(4500), Instant::now());
        let fired = run_for(&mut s, Duration::from_secs(150)).await;
        // 80% of half the 300 s device timeout.
        assert_eq!(fired[0].0, Duration::from_secs(120));
    }

    #[tokio::test(start_paused = true)]
    async fn goodbye_cancels_refresh() {
        let mut s = sched();
        s.take_due(Instant::now());
        s.meta.next = Instant::now() + Duration::from_secs(10_000);
        s.on_record(&a_rec(120), Instant::now());
        s.on_record(&a_rec(0), Instant::now());
        assert!(s.refresh.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn refinement_is_immediate_and_deduplicated() {
        let mut s = sched();
        s.take_due(Instant::now());
        let req = QueryRequest {
            name: DnsName::parse("P._ipp._tcp.local"),
            rtypes: vec![rtype::SRV, rtype::TXT],
        };
        s.on_request(&req, Instant::now());
        assert_eq!(s.next_due(), Some(Instant::now()));
        assert_eq!(
            s.take_due(Instant::now()),
            vec![q("P._ipp._tcp.local", rtype::TXT), q("P._ipp._tcp.local", rtype::SRV)]
        );
        advance(Duration::from_millis(500)).await;
        s.on_request(&req, Instant::now());
        assert!(s.immediate.is_empty(), "duplicate within 1 s ignored");
        advance(Duration::from_millis(600)).await;
        s.on_request(&req, Instant::now());
        assert_eq!(s.immediate.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn questions_due_together_are_batched() {
        let mut s = sched();
        s.take_due(Instant::now());
        s.meta.next = Instant::now() + Duration::from_secs(10_000);
        s.on_record(&a_rec(120), Instant::now());
        advance(Duration::from_millis(10)).await;
        s.on_record(
            &rec("p.local", RData::Aaaa("fe80::1".parse().unwrap()), 120),
            Instant::now(),
        );
        advance(Duration::from_secs(96)).await;
        assert_eq!(s.take_due(Instant::now()).len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn known_answers_need_half_ttl_left() {
        let mut s = sched();
        let meta = "_services._dns-sd._udp.local";
        s.on_record(&rec(meta, RData::Ptr(DnsName::parse("_ipp._tcp.local")), 100), Instant::now());
        let qs = vec![q(meta, rtype::PTR)];
        advance(Duration::from_secs(40)).await;
        let known = s.known_answers(&qs, Instant::now());
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].ttl, 60);
        advance(Duration::from_secs(20)).await;
        assert!(s.known_answers(&qs, Instant::now()).is_empty());
        assert!(s.known_answers(&[q("_ipp._tcp.local", rtype::PTR)], Instant::now()).is_empty());
    }
}
