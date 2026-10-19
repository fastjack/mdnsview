//! Store and snapshot tests on virtual time.

use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, SystemTime};

use tokio::time::{Instant, advance};

use super::record::{
    DnsName, Observation, ObservedRecord, QueryRequest, RData, Source, SourceState, rtype,
};
use super::snapshot::{Liveness, Snapshot, UNRESOLVED_ID};
use super::store::{Store, spawn};

const TIMEOUT: Duration = Duration::from_secs(300);

fn name(s: &str) -> DnsName {
    DnsName::parse(s)
}

fn rec(source: Source, owner: &str, data: RData, ttl: Option<u32>) -> Observation {
    Observation::Record(ObservedRecord {
        name: name(owner),
        data,
        ttl,
        cache_flush: false,
        source,
        src_addr: Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40))),
        iface: Some("wlan0".into()),
        at: Instant::now(),
        wall: SystemTime::now(),
    })
}

fn wire(owner: &str, data: RData, ttl: u32) -> Observation {
    rec(Source::Wire, owner, data, Some(ttl))
}

fn ptr(target: &str) -> RData {
    RData::Ptr(name(target))
}

fn srv(target: &str, port: u16) -> RData {
    RData::Srv {
        priority: 0,
        weight: 0,
        port,
        target: name(target),
    }
}

fn txt(entries: &[&str]) -> RData {
    RData::Txt(entries.iter().map(|e| e.as_bytes().to_vec()).collect())
}

fn a(last: u8) -> RData {
    RData::A(Ipv4Addr::new(192, 168, 1, last))
}

/// Feeds a complete wire advertisement of one instance.
fn advertise(store: &mut Store, instance: &str, host: &str, port: u16, ttl: u32) {
    let inst = name(instance);
    let ty = inst.parent().to_string();
    store.ingest(wire(&ty, ptr(instance), ttl));
    store.ingest(wire(instance, srv(host, port), ttl));
    store.ingest(wire(instance, txt(&["txtvers=1"]), ttl));
}

fn snap(store: &Store) -> Snapshot {
    store.snapshot(Instant::now())
}

fn device_labels(s: &Snapshot) -> Vec<&str> {
    s.devices.iter().map(|d| d.label.as_str()).collect()
}

#[tokio::test(start_paused = true)]
async fn services_grouped_by_srv_target() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "Office Printer._ipp._tcp.local", "brother-a1b2.local", 631, 4500);
    advertise(&mut store, "Office Printer._http._tcp.local", "brother-a1b2.local", 80, 4500);
    advertise(&mut store, "Office Printer._printer._tcp.local", "brother-a1b2.local", 515, 4500);
    store.ingest(wire("brother-a1b2.local", a(40), 120));

    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    let d = &s.devices[0];
    assert_eq!(d.id, "brother-a1b2.local");
    assert_eq!(d.services.len(), 3);
    assert_eq!(d.addresses, vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40))]);
    let ipp = &s.services["office printer._ipp._tcp.local"];
    assert_eq!(ipp.port, Some(631));
    assert_eq!(ipp.type_label, "Printer (IPP)");
    assert_eq!(ipp.addresses, d.addresses);
}

#[tokio::test(start_paused = true)]
async fn shared_address_with_different_labels_does_not_merge() {
    // Host-only devices are labelled by host name, so the labels differ.
    let mut store = Store::new(TIMEOUT);
    store.ingest(wire("nas.local", a(10), 120));
    store.ingest(wire("nas-2.local", a(10), 120));
    assert_eq!(snap(&store).devices.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn host_only_device() {
    let mut store = Store::new(TIMEOUT);
    store.ingest(wire("router.local", a(1), 120));
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    assert_eq!(s.devices[0].id, "router.local");
    assert!(s.devices[0].services.is_empty());
    assert_eq!(s.devices[0].label, "router.local");
}

#[tokio::test(start_paused = true)]
async fn instance_without_srv_is_unresolved_until_srv_arrives() {
    let mut store = Store::new(TIMEOUT);
    store.ingest(wire("_ipp._tcp.local", ptr("Printer._ipp._tcp.local"), 4500));
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    assert_eq!(s.devices[0].id, UNRESOLVED_ID);
    assert_eq!(s.devices[0].label, "(unresolved)");

    store.ingest(wire("Printer._ipp._tcp.local", srv("p.local", 631), 120));
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    assert_eq!(s.devices[0].id, "p.local");
    assert_eq!(s.devices[0].services, vec!["printer._ipp._tcp.local"]);
}

#[tokio::test(start_paused = true)]
async fn label_uses_majority_instance_name() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "Office Printer._ipp._tcp.local", "b.local", 631, 4500);
    advertise(&mut store, "Office Printer._printer._tcp.local", "b.local", 515, 4500);
    advertise(&mut store, "Brother Web._http._tcp.local", "b.local", 80, 4500);
    assert_eq!(device_labels(&snap(&store)), vec!["Office Printer"]);
}

#[tokio::test(start_paused = true)]
async fn label_prefers_instance_name_over_model() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "Living Room._airplay._tcp.local", "tv.local", 7000, 4500);
    store.ingest(wire(
        "Living Room._device-info._tcp.local",
        txt(&["model=AppleTV11,1"]),
        4500,
    ));
    let s = snap(&store);
    assert_eq!(device_labels(&s), vec!["Living Room"]);
    // The device-info pseudo-instance is not listed as a service.
    assert_eq!(s.services.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn label_uses_model_when_device_has_no_services() {
    let mut store = Store::new(TIMEOUT);
    store.ingest(wire("Hyperion.local", a(147), 120));
    store.ingest(wire("Hyperion._device-info._tcp.local", txt(&["model=Mac14,10"]), 4500));
    assert_eq!(device_labels(&snap(&store)), vec!["Mac14,10"]);
}

#[tokio::test(start_paused = true)]
async fn service_types_with_readable_names() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "TV._airplay._tcp.local", "tv.local", 7000, 4500);
    advertise(&mut store, "X._foo-bar._tcp.local", "x.local", 1, 4500);
    let s = snap(&store);
    let labels: Vec<_> = s.types.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(labels, vec!["_foo-bar._tcp", "AirPlay"]);
    assert_eq!(s.service_type("_airplay._tcp").unwrap().instances.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn txt_entries_are_sorted() {
    let mut store = Store::new(TIMEOUT);
    store.ingest(wire("P._ipp._tcp.local", srv("p.local", 631), 120));
    store.ingest(wire("P._ipp._tcp.local", txt(&["ty=Brother", "note=2nd floor", "Color"]), 4500));
    let s = snap(&store);
    let keys: Vec<_> = s.services["p._ipp._tcp.local"].txt.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["Color", "note", "ty"]);
}

#[tokio::test(start_paused = true)]
async fn provenance_both() {
    let mut store = Store::new(TIMEOUT);
    store.ingest(rec(Source::System, "P._ipp._tcp.local", txt(&["a=1"]), None));
    store.ingest(wire("P._ipp._tcp.local", txt(&["a=1"]), 4500));
    store.ingest(wire("P._ipp._tcp.local", srv("p.local", 631), 120));
    let s = snap(&store);
    let txt_view = s.services["p._ipp._tcp.local"]
        .records
        .iter()
        .find(|r| r.data.rtype() == rtype::TXT)
        .unwrap();
    assert_eq!(txt_view.sources.label(), "both");
}

#[tokio::test(start_paused = true)]
async fn last_seen_follows_refresh_answers() {
    let mut store = Store::new(TIMEOUT);
    store.ingest(wire("router.local", a(1), 120));
    advance(Duration::from_secs(96)).await;
    store.ingest(wire("router.local", a(1), 120));
    let s = snap(&store);
    assert_eq!(s.devices[0].last_seen, Instant::now());
    assert_eq!(s.devices[0].state, Liveness::Live);
}

#[tokio::test(start_paused = true)]
async fn records_expire_to_stale_and_return_to_live() {
    let mut store = Store::new(TIMEOUT);
    store.ingest(wire("router.local", a(1), 120));
    advance(Duration::from_secs(121)).await;
    assert_eq!(snap(&store).devices[0].state, Liveness::Stale);
    store.ingest(wire("router.local", a(1), 120));
    let s = snap(&store);
    assert_eq!(s.devices[0].state, Liveness::Live);
    assert_eq!(s.devices[0].id, "router.local");
}

#[tokio::test(start_paused = true)]
async fn goodbye_marks_left() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "P._ipp._tcp.local", "p.local", 631, 120);
    store.ingest(wire("P._ipp._tcp.local", srv("p.local", 631), 0));
    store.ingest(wire("_ipp._tcp.local", ptr("P._ipp._tcp.local"), 0));
    store.ingest(wire("P._ipp._tcp.local", txt(&["txtvers=1"]), 0));
    let s = snap(&store);
    assert_eq!(s.services["p._ipp._tcp.local"].state, Liveness::Left);
    assert_eq!(s.devices[0].state, Liveness::Left);
}

#[tokio::test(start_paused = true)]
async fn silent_device_removed_after_timeout() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "P._ipp._tcp.local", "p.local", 631, 120);
    store.ingest(wire("p.local", a(40), 120));
    advance(Duration::from_secs(300)).await;
    store.gc(Instant::now());
    assert_eq!(snap(&store).devices.len(), 1, "still listed at exactly the timeout");
    advance(Duration::from_secs(1)).await;
    store.gc(Instant::now());
    let s = snap(&store);
    assert!(s.devices.is_empty());
    assert!(s.types.is_empty());
    assert_eq!(store.record_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn custom_timeout() {
    let mut store = Store::new(Duration::from_secs(600));
    store.ingest(wire("router.local", a(1), 120));
    advance(Duration::from_secs(360)).await;
    store.gc(Instant::now());
    assert_eq!(snap(&store).devices.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn retained_device_kept_until_released() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "P._ipp._tcp.local", "p.local", 631, 120);
    store.ingest(wire("p.local", a(40), 120));
    store.set_retained(["p.local".to_string()]);
    advance(Duration::from_secs(400)).await;
    store.gc(Instant::now());
    let s = snap(&store);
    let d = &s.devices[0];
    assert!(d.retained && d.gone);
    assert_eq!(d.services.len(), 1, "details of a retained device stay available");

    store.set_retained([]);
    store.gc(Instant::now());
    assert!(snap(&store).devices.is_empty());
}

#[tokio::test(start_paused = true)]
async fn expired_service_of_live_device_is_removed() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "P._ipp._tcp.local", "p.local", 631, 120);
    store.set_retained(["p.local".to_string()]);
    for _ in 0..4 {
        advance(Duration::from_secs(100)).await;
        store.ingest(wire("p.local", a(40), 120));
    }
    store.gc(Instant::now());
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    assert!(s.devices[0].services.is_empty());
}

#[tokio::test(start_paused = true)]
async fn retained_service_keeps_its_records() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "P._ipp._tcp.local", "p.local", 631, 120);
    store.set_retained(["p._ipp._tcp.local".to_string(), "p.local".to_string()]);
    advance(Duration::from_secs(400)).await;
    store.gc(Instant::now());
    let s = snap(&store);
    assert!(s.services["p._ipp._tcp.local"].retained);
    assert_eq!(s.devices.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn cache_flush_expires_older_siblings() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "P._ipp._tcp.local", "p.local", 631, 4500);
    store.ingest(wire("p.local", a(40), 120));
    advance(Duration::from_secs(5)).await;
    let Observation::Record(mut fresh) = wire("p.local", a(41), 120) else {
        unreachable!()
    };
    fresh.cache_flush = true;
    store.ingest(Observation::Record(fresh));
    advance(Duration::from_millis(1500)).await;
    let s = snap(&store);
    let expired: Vec<_> = s.services["p._ipp._tcp.local"]
        .records
        .iter()
        .filter(|r| r.data.rtype() == rtype::A)
        .map(|r| (r.data.address().unwrap(), r.expired))
        .collect();
    assert_eq!(
        expired,
        vec![
            (IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40)), true),
            (IpAddr::V4(Ipv4Addr::new(192, 168, 1, 41)), false),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn cache_flush_spares_records_from_same_burst() {
    let mut store = Store::new(TIMEOUT);
    store.ingest(wire("p.local", a(40), 120));
    let Observation::Record(mut fresh) = wire("p.local", a(41), 120) else {
        unreachable!()
    };
    fresh.cache_flush = true;
    store.ingest(Observation::Record(fresh));
    advance(Duration::from_secs(60)).await;
    store.ingest(wire("q.local", a(1), 120)); // unrelated
    let s = snap(&store);
    let d = s.device("p.local").unwrap();
    assert_eq!(d.state, Liveness::Live);
    assert_eq!(d.addresses.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn system_records_held_until_removed() {
    let mut store = Store::new(TIMEOUT);
    store.ingest(Observation::SourceStatus {
        source: Source::Wire,
        state: SourceState::Unavailable("bind failed".into()),
    });
    store.ingest(rec(Source::System, "_ipp._tcp.local", ptr("P._ipp._tcp.local"), None));
    store.ingest(rec(Source::System, "P._ipp._tcp.local", srv("p.local", 631), None));
    store.ingest(rec(Source::System, "p.local", a(40), None));
    advance(Duration::from_secs(1000)).await;
    store.gc(Instant::now());
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1, "system-only: daemon-held entries count as seen");
    assert_eq!(s.devices[0].last_seen, Instant::now());

    store.ingest(Observation::SystemRemoved {
        instance: name("P._ipp._tcp.local"),
        at: Instant::now(),
    });
    let s = snap(&store);
    assert_eq!(s.devices[0].state, Liveness::Left);
    advance(Duration::from_secs(301)).await;
    store.gc(Instant::now());
    assert!(snap(&store).devices.is_empty());
}

#[tokio::test(start_paused = true)]
async fn refinement_requested_for_daemon_only_instances() {
    let mut store = Store::new(TIMEOUT);
    let q = store.ingest(rec(Source::System, "_ipp._tcp.local", ptr("P._ipp._tcp.local"), None));
    assert_eq!(
        q,
        vec![QueryRequest {
            name: name("P._ipp._tcp.local"),
            rtypes: vec![rtype::SRV, rtype::TXT],
        }]
    );
    // Throttled while the first request is recent.
    let q = store.ingest(rec(Source::System, "_ipp._tcp.local", ptr("P._ipp._tcp.local"), None));
    assert!(q.is_empty());

    let q = store.ingest(rec(Source::System, "P._ipp._tcp.local", srv("p.local", 631), None));
    assert_eq!(q[0].name, name("p.local"));
    assert_eq!(q[0].rtypes, vec![rtype::A, rtype::AAAA]);

    // Once the wire has the SRV record, no more requests for the instance.
    store.ingest(wire("P._ipp._tcp.local", srv("p.local", 631), 120));
    advance(Duration::from_secs(31)).await;
    let q = store.ingest(rec(Source::System, "_ipp._tcp.local", ptr("P._ipp._tcp.local"), None));
    assert!(q.is_empty());
}

#[tokio::test(start_paused = true)]
async fn store_task_publishes_snapshots_and_queries() {
    let (qtx, mut qrx) = tokio::sync::mpsc::unbounded_channel();
    let (mut handle, _task) = spawn(TIMEOUT, Some(qtx));
    handle
        .observations
        .send(rec(Source::System, "_ipp._tcp.local", ptr("P._ipp._tcp.local"), None))
        .await
        .unwrap();
    handle.snapshots.changed().await.unwrap();
    // Allow the coalescing interval to pass.
    advance(Duration::from_millis(300)).await;
    handle.snapshots.changed().await.unwrap();
    let s = handle.snapshots.borrow_and_update().clone();
    assert_eq!(s.services.len(), 1);
    assert_eq!(qrx.recv().await.unwrap().name, name("P._ipp._tcp.local"));

    // Retention changes publish immediately.
    handle.retain.send(vec![UNRESOLVED_ID.to_string()]).unwrap();
    handle.snapshots.changed().await.unwrap();
    assert!(handle.snapshots.borrow().devices[0].retained);
}

fn aaaa(text: &str) -> RData {
    RData::Aaaa(text.parse().unwrap())
}

const SERVICE_TYPES: [&str; 8] = ["_airplay", "_companion-link", "_rfb", "_smb", "_ssh", "_sftp-ssh", "_raop", "_net-assistant"];

/// The office Mac: 8 services on Hyperion.local, 1 on a UUID host name.
fn hyperion(store: &mut Store, ttl: u32) {
    for t in SERVICE_TYPES {
        advertise(store, &format!("Hyperion.{t}._tcp.local"), "Hyperion.local", 1000, ttl);
    }
    advertise(store, "Hyperion._abm-mh-v2._tcp.local", "1e8da75b.local", 2000, ttl);
    for host in ["Hyperion.local", "1e8da75b.local"] {
        store.ingest(wire(host, a(147), ttl.min(120)));
        store.ingest(wire(host, aaaa("fe80::c05:d0d4:84b1:b726"), ttl.min(120)));
    }
}

#[tokio::test(start_paused = true)]
async fn one_machine_under_two_host_names_is_merged() {
    let mut store = Store::new(TIMEOUT);
    hyperion(&mut store, 4500);
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1, "{:?}", device_labels(&s));
    let d = &s.devices[0];
    assert_eq!(d.label, "Hyperion");
    assert_eq!(d.id, "hyperion.local");
    assert_eq!(d.services.len(), 9);
    assert_eq!(d.hosts, vec![name("Hyperion.local"), name("1e8da75b.local")]);
    assert_eq!(d.addresses.len(), 2);
    assert!(s.services.values().all(|svc| svc.device_id == "hyperion.local"));
}

#[tokio::test(start_paused = true)]
async fn same_name_different_addresses_stay_separate() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "Office Printer._ipp._tcp.local", "printer-a.local", 631, 4500);
    advertise(&mut store, "Office Printer._http._tcp.local", "printer-b.local", 80, 4500);
    store.ingest(wire("printer-a.local", a(40), 120));
    store.ingest(wire("printer-b.local", a(41), 120));
    assert_eq!(device_labels(&snap(&store)), vec!["Office Printer", "Office Printer"]);
}

#[tokio::test(start_paused = true)]
async fn merge_waits_for_a_shared_address() {
    let mut store = Store::new(TIMEOUT);
    advertise(&mut store, "Hyperion._ssh._tcp.local", "Hyperion.local", 22, 4500);
    advertise(&mut store, "Hyperion._smb._tcp.local", "Hyperion.local", 445, 4500);
    advertise(&mut store, "Hyperion._abm-mh-v2._tcp.local", "1e8da75b.local", 2000, 4500);
    store.ingest(wire("Hyperion.local", a(147), 120));
    let s = snap(&store);
    assert_eq!(s.devices.len(), 2);
    assert!(s.device("hyperion.local").is_some() && s.device("1e8da75b.local").is_some());

    store.ingest(wire("1e8da75b.local", a(147), 120));
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    assert_eq!(s.devices[0].id, "hyperion.local", "id of the larger part is kept");
}

#[tokio::test(start_paused = true)]
async fn merging_is_transitive() {
    let mut store = Store::new(TIMEOUT);
    for (i, host) in ["x1.local", "x2.local", "x3.local"].iter().enumerate() {
        let t = ["_http", "_ssh", "_smb"][i];
        advertise(&mut store, &format!("Box.{t}._tcp.local"), host, 1, 4500);
        store.ingest(wire(host, a(10 + i as u8), 120));
        store.ingest(wire(host, a(11 + i as u8), 120));
    }
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    assert_eq!(s.devices[0].hosts.len(), 3);
}

#[tokio::test(start_paused = true)]
async fn unresolved_bucket_is_never_merged() {
    let mut store = Store::new(TIMEOUT);
    store.ingest(wire("_ipp._tcp.local", ptr("Lost._ipp._tcp.local"), 4500));
    advertise(&mut store, "(unresolved)._http._tcp.local", "odd.local", 80, 4500);
    store.ingest(wire("odd.local", a(5), 120));
    let s = snap(&store);
    assert_eq!(s.devices.len(), 2);
    assert!(s.device(UNRESOLVED_ID).is_some());
}

#[tokio::test(start_paused = true)]
async fn merged_last_seen_is_the_latest_host() {
    let mut store = Store::new(TIMEOUT);
    hyperion(&mut store, 120);
    advance(Duration::from_secs(110)).await;
    store.ingest(wire("1e8da75b.local", a(147), 120));
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    assert_eq!(s.devices[0].last_seen, Instant::now());
}

#[tokio::test(start_paused = true)]
async fn merged_device_live_while_one_host_is_live() {
    let mut store = Store::new(TIMEOUT);
    hyperion(&mut store, 120);
    advance(Duration::from_secs(100)).await;
    store.ingest(wire("1e8da75b.local", a(147), 120));
    advance(Duration::from_secs(30)).await; // Hyperion.local's records have expired
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    let d = &s.devices[0];
    assert_eq!(d.state, Liveness::Live);
    let main_host = s.services["hyperion._ssh._tcp.local"].state;
    assert_eq!(main_host, Liveness::Stale);
}

#[tokio::test(start_paused = true)]
async fn retention_through_a_member_host_key() {
    let mut store = Store::new(TIMEOUT);
    hyperion(&mut store, 4500);
    store.set_retained(["1e8da75b.local".to_string()]);
    assert!(snap(&store).devices[0].retained);
}

#[tokio::test(start_paused = true)]
async fn address_change_splits_the_device() {
    let mut store = Store::new(TIMEOUT);
    hyperion(&mut store, 4500);
    advance(Duration::from_secs(5)).await;
    for data in [a(200), aaaa("fe80::1")] {
        let Observation::Record(mut fresh) = wire("1e8da75b.local", data, 120) else {
            unreachable!()
        };
        fresh.cache_flush = true;
        store.ingest(Observation::Record(fresh));
    }
    advance(Duration::from_secs(2)).await;
    let s = snap(&store);
    assert_eq!(s.devices.len(), 2);
    assert_eq!(s.device("hyperion.local").unwrap().services.len(), 8, "primary keeps its id");
    assert_eq!(s.device("1e8da75b.local").unwrap().services.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn stale_merged_device_stays_merged() {
    let mut store = Store::new(TIMEOUT);
    hyperion(&mut store, 4500);
    advance(Duration::from_secs(200)).await; // address records (ttl 120) expired
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    assert_eq!(s.devices[0].hosts.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn silent_host_name_leaves_merged_device() {
    let mut store = Store::new(TIMEOUT);
    hyperion(&mut store, 120);
    // Only Hyperion.local keeps answering.
    for _ in 0..4 {
        advance(Duration::from_secs(100)).await;
        hyperion_main_answers(&mut store);
    }
    store.gc(Instant::now());
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    let d = &s.devices[0];
    assert_eq!(d.hosts, vec![name("Hyperion.local")]);
    assert_eq!(d.services.len(), 8);
    assert!(!s.services.contains_key("hyperion._abm-mh-v2._tcp.local"));
}

fn hyperion_main_answers(store: &mut Store) {
    for t in SERVICE_TYPES {
        advertise(store, &format!("Hyperion.{t}._tcp.local"), "Hyperion.local", 1000, 120);
    }
    store.ingest(wire("Hyperion.local", a(147), 120));
}

#[tokio::test(start_paused = true)]
async fn selected_primary_host_is_kept_while_the_rest_answers() {
    let mut store = Store::new(TIMEOUT);
    hyperion(&mut store, 120);
    store.set_retained(["hyperion.local".to_string()]);
    // Only the secondary host keeps answering.
    for _ in 0..4 {
        advance(Duration::from_secs(100)).await;
        advertise(&mut store, "Hyperion._abm-mh-v2._tcp.local", "1e8da75b.local", 2000, 120);
        store.ingest(wire("1e8da75b.local", a(147), 120));
    }
    store.gc(Instant::now());
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    assert_eq!(s.devices[0].id, "hyperion.local", "selection keeps its device id");
    assert_eq!(s.devices[0].services.len(), 9, "the held host keeps its services");
}

#[tokio::test(start_paused = true)]
async fn retained_gone_merged_device_kept_whole_until_released() {
    let mut store = Store::new(TIMEOUT);
    hyperion(&mut store, 120);
    store.set_retained(["hyperion.local".to_string()]);
    advance(Duration::from_secs(400)).await;
    store.gc(Instant::now());
    let s = snap(&store);
    assert_eq!(s.devices.len(), 1);
    assert!(s.devices[0].gone && s.devices[0].retained);
    assert_eq!(s.devices[0].services.len(), 9);
    assert_eq!(s.devices[0].hosts.len(), 2);

    store.set_retained([]);
    store.gc(Instant::now());
    assert!(snap(&store).devices.is_empty());
    assert_eq!(store.record_count(), 0);
}
