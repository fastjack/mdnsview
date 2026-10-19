//! Key handling and rendering tests on a ratatui `TestBackend`.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use tokio::time::{Instant, advance};

use super::app::{App, DetailMode, Focus, Mode};
use super::view;
use crate::model::record::{DnsName, Observation, ObservedRecord, RData, Source, SourceState};
use crate::model::store::Store;

const TIMEOUT: Duration = Duration::from_secs(300);

fn wire(owner: &str, data: RData, ttl: u32) -> Observation {
    Observation::Record(ObservedRecord {
        name: DnsName::parse(owner),
        data,
        ttl: Some(ttl),
        cache_flush: true,
        source: Source::Wire,
        src_addr: Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40))),
        iface: Some("wlan0".into()),
        at: Instant::now(),
        wall: SystemTime::now(),
    })
}

fn advertise(store: &mut Store, name: &str, ty: &str, host: &str, port: u16, txt: &[&[u8]]) {
    let instance = format!("{name}.{ty}.local");
    store.ingest(wire(&format!("{ty}.local"), RData::Ptr(DnsName::parse(&instance)), 4500));
    store.ingest(wire(
        &instance,
        RData::Srv {
            priority: 0,
            weight: 0,
            port,
            target: DnsName::parse(host),
        },
        120,
    ));
    store.ingest(wire(&instance, RData::Txt(txt.iter().map(|t| t.to_vec()).collect()), 4500));
}

fn address(store: &mut Store, host: &str, last: u8) {
    store.ingest(wire(host, RData::A(Ipv4Addr::new(192, 168, 1, last)), 120));
}

/// A printer, a NAS and a TV.
fn lan() -> Store {
    let mut store = Store::new(TIMEOUT);
    for status in [
        (Source::System, SourceState::Active),
        (Source::Wire, SourceState::Active),
    ] {
        store.ingest(Observation::SourceStatus {
            source: status.0,
            state: status.1,
        });
    }
    store.ingest(Observation::Interfaces(vec!["wlan0".into()]));
    advertise(&mut store, "Office Printer", "_ipp._tcp", "brother.local", 631, &[b"ty=Brother HL", b"note=2nd floor", b"bin=\x01\x02"]);
    advertise(&mut store, "Office Printer", "_http._tcp", "brother.local", 80, &[]);
    address(&mut store, "brother.local", 40);
    advertise(&mut store, "NAS", "_smb._tcp", "nas.local", 445, &[]);
    address(&mut store, "nas.local", 10);
    advertise(&mut store, "Living Room", "_airplay._tcp", "tv.local", 7000, &[]);
    address(&mut store, "tv.local", 20);
    store
}

fn app(store: &Store) -> App {
    App::new(Arc::new(store.snapshot(Instant::now())))
}

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn keys(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn render(app: &App) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
    terminal.draw(|f| view::render(f, app, "avahi")).unwrap();
    terminal
}

fn screen(app: &App) -> String {
    let terminal = render(app);
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// Foreground colour of the first cell of the line containing `text`.
fn fg_of(app: &App, text: &str) -> Color {
    let terminal = render(app);
    let buffer = terminal.backend().buffer();
    let screen = screen(app);
    let (y, line) = screen.lines().enumerate().find(|(_, l)| l.contains(text)).unwrap();
    let x = line[..line.find(text).unwrap()].chars().count() as u16;
    buffer[(x, y as u16)].fg
}

fn left_labels(app: &App) -> Vec<String> {
    app.left_items().iter().map(|i| i.id().to_string()).collect()
}

#[tokio::test(start_paused = true)]
async fn navigation_keys() {
    let store = lan();
    let mut a = app(&store);
    assert_eq!(a.focus, Focus::Left);
    assert_eq!(left_labels(&a), vec!["tv.local", "nas.local", "brother.local"]);
    assert_eq!(a.left.as_deref(), Some("tv.local"));
    key(&mut a, KeyCode::Char('j'));
    assert_eq!(a.left.as_deref(), Some("nas.local"));
    key(&mut a, KeyCode::Down);
    key(&mut a, KeyCode::Down);
    assert_eq!(a.left.as_deref(), Some("brother.local"), "stops at the end");
    key(&mut a, KeyCode::Char('k'));
    key(&mut a, KeyCode::Up);
    assert_eq!(a.left.as_deref(), Some("tv.local"));

    key(&mut a, KeyCode::Char('l'));
    assert_eq!(a.focus, Focus::Middle);
    key(&mut a, KeyCode::Right);
    assert_eq!(a.focus, Focus::Right);
    key(&mut a, KeyCode::Char('h'));
    key(&mut a, KeyCode::Left);
    assert_eq!(a.focus, Focus::Left);

    key(&mut a, KeyCode::Char('d'));
    assert_eq!(a.detail, DetailMode::Raw);
    key(&mut a, KeyCode::Char('d'));
    assert_eq!(a.detail, DetailMode::Simple);

    key(&mut a, KeyCode::Tab);
    assert_eq!(a.mode, Mode::Types);
    key(&mut a, KeyCode::Tab);
    assert_eq!(a.mode, Mode::Devices);

    assert!(!a.quit);
    key(&mut a, KeyCode::Char('q'));
    assert!(a.quit);
}

#[tokio::test(start_paused = true)]
async fn ctrl_c_quits() {
    let store = lan();
    let mut a = app(&store);
    a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(a.quit);
}

#[tokio::test(start_paused = true)]
async fn device_mode_drill_down() {
    let store = lan();
    let mut a = app(&store);
    keys(&mut a, "jj"); // brother.local
    let s = screen(&a);
    assert!(s.contains("Office Printer"), "{s}");
    // The middle pane lists the device's services by type.
    assert!(s.contains("Printer (IPP)"), "{s}");
    assert!(s.contains("Web server (HTTP)"), "{s}");
    // Relative last seen in the device list.
    assert!(s.contains("now"), "{s}");
}

#[tokio::test(start_paused = true)]
async fn relative_times() {
    assert_eq!(view::relative(Duration::from_millis(500)), "now");
    assert_eq!(view::relative(Duration::from_secs(14)), "14s");
    assert_eq!(view::relative(Duration::from_secs(200)), "3m");
    assert_eq!(view::relative(Duration::from_secs(7300)), "2h");
}

#[tokio::test(start_paused = true)]
async fn device_row_shows_age_and_departed_rows_are_dimmed() {
    let mut store = lan();
    advance(Duration::from_secs(200)).await;
    address(&mut store, "nas.local", 10);
    // The TV says goodbye for all its records.
    let instance = "Living Room._airplay._tcp.local";
    store.ingest(wire("_airplay._tcp.local", RData::Ptr(DnsName::parse(instance)), 0));
    store.ingest(wire(
        instance,
        RData::Srv {
            priority: 0,
            weight: 0,
            port: 7000,
            target: DnsName::parse("tv.local"),
        },
        0,
    ));
    store.ingest(wire(instance, RData::Txt(vec![]), 0));
    store.ingest(wire("tv.local", RData::A(Ipv4Addr::new(192, 168, 1, 20)), 0));
    let a = app(&store);
    let s = screen(&a);
    let printer = s.lines().find(|l| l.contains("Office Printer")).unwrap();
    assert!(printer.contains("3m"), "live device seen 200 s ago: {printer}");
    let tv = s.lines().find(|l| l.contains("Living Room")).unwrap();
    assert!(tv.contains("left now"), "{tv}");
    assert_eq!(fg_of(&a, "Living Room"), Color::DarkGray);
    assert_ne!(fg_of(&a, "NAS"), Color::DarkGray);
    assert_ne!(fg_of(&a, "Office Printer"), Color::DarkGray);
}

#[tokio::test(start_paused = true)]
async fn row_inserted_above_selection_keeps_selection() {
    let mut store = lan();
    let mut a = app(&store);
    key(&mut a, KeyCode::Char('j'));
    assert_eq!(a.left.as_deref(), Some("nas.local"));
    advertise(&mut store, "Apple TV", "_airplay._tcp", "atv.local", 7000, &[]);
    a.set_snapshot(Arc::new(store.snapshot(Instant::now())));
    assert_eq!(left_labels(&a)[0], "atv.local");
    assert_eq!(a.left.as_deref(), Some("nas.local"));
}

#[tokio::test(start_paused = true)]
async fn service_type_pivot() {
    let mut store = lan();
    advertise(&mut store, "Lobby Printer", "_ipp._tcp", "lobby.local", 631, &[]);
    let mut a = app(&store);
    key(&mut a, KeyCode::Tab);
    assert_eq!(a.mode, Mode::Types);
    let pos = left_labels(&a).iter().position(|t| t == "_ipp._tcp").unwrap();
    for _ in 0..pos {
        key(&mut a, KeyCode::Down);
    }
    let s = screen(&a);
    assert!(s.contains("Service types"), "{s}");
    assert!(s.contains("Printer (IPP)"), "{s}");
    assert!(s.contains("Office Printer @brother.local"), "{s}");
    assert!(s.contains("Lobby Printer @lobby.local"), "{s}");
}

#[tokio::test(start_paused = true)]
async fn pivot_keeps_selected_service() {
    let store = lan();
    let mut a = app(&store);
    keys(&mut a, "jjl"); // brother.local, focus services
    key(&mut a, KeyCode::Down); // second service
    let selected = a.middle.clone().unwrap();
    key(&mut a, KeyCode::Tab);
    assert_eq!(a.middle.as_ref(), Some(&selected));
    assert_eq!(a.left.as_deref(), Some(a.selected_service().unwrap().service_type.as_str()));
    key(&mut a, KeyCode::Tab);
    assert_eq!(a.left.as_deref(), Some("brother.local"));
    assert_eq!(a.middle.as_ref(), Some(&selected));
}

#[tokio::test(start_paused = true)]
async fn filter_in_device_mode() {
    let mut store = lan();
    advertise(&mut store, "printer-lab", "_ssh._tcp", "lab.local", 22, &[]);
    let mut a = app(&store);
    key(&mut a, KeyCode::Char('/'));
    keys(&mut a, "printer");
    // Matches by readable type name (brother) and by label (lab).
    let mut ids = left_labels(&a);
    ids.sort();
    assert_eq!(ids, vec!["brother.local", "lab.local"]);
    key(&mut a, KeyCode::Enter);
    assert!(!a.editing_filter);
    assert!(screen(&a).contains("/printer"));
    // Esc clears the filter.
    key(&mut a, KeyCode::Esc);
    assert_eq!(left_labels(&a).len(), 4);
}

#[tokio::test(start_paused = true)]
async fn filter_in_type_mode_matches_hosts() {
    let store = lan();
    let mut a = app(&store);
    key(&mut a, KeyCode::Tab);
    key(&mut a, KeyCode::Char('/'));
    keys(&mut a, "brother");
    let mut ids = left_labels(&a);
    ids.sort();
    assert_eq!(ids, vec!["_http._tcp", "_ipp._tcp"]);
    key(&mut a, KeyCode::Esc);
    assert_eq!(left_labels(&a).len(), 4);
}

#[tokio::test(start_paused = true)]
async fn filter_hiding_selection_moves_it_and_releases_retention() {
    let mut store = lan();
    let mut a = app(&store);
    key(&mut a, KeyCode::Char('j')); // nas.local
    store.set_retained(a.retained_ids());
    advance(Duration::from_secs(400)).await;
    address(&mut store, "brother.local", 40);
    store.gc(Instant::now());
    a.set_snapshot(Arc::new(store.snapshot(Instant::now())));
    assert!(left_labels(&a).contains(&"nas.local".to_string()), "retained while selected");

    key(&mut a, KeyCode::Char('/'));
    keys(&mut a, "brother");
    assert_eq!(a.left.as_deref(), Some("brother.local"));
    store.set_retained(a.retained_ids());
    store.gc(Instant::now());
    key(&mut a, KeyCode::Esc);
    a.set_snapshot(Arc::new(store.snapshot(Instant::now())));
    assert_eq!(left_labels(&a), vec!["brother.local"], "released entries are removed");
}

#[tokio::test(start_paused = true)]
async fn simple_details_view() {
    let store = lan();
    let mut a = app(&store);
    keys(&mut a, "jjl");
    let s = screen(&a);
    assert!(s.contains("Office Printer"), "{s}");
    assert!(s.contains("Printer (IPP) (_ipp._tcp)"), "{s}");
    assert!(s.contains("brother.local"), "{s}");
    assert!(s.contains("192.168.1.40"), "{s}");
    assert!(s.contains("631"), "{s}");
    // TXT keys sorted.
    let note = s.find("note").unwrap();
    let ty = s.find("ty   ").unwrap();
    assert!(note < ty, "{s}");
    assert!(s.contains("2nd floor"), "{s}");
}

#[tokio::test(start_paused = true)]
async fn raw_records_view() {
    let mut store = lan();
    advance(Duration::from_secs(10)).await;
    address(&mut store, "brother.local", 41);
    let mut a = app(&store);
    keys(&mut a, "jjld");
    let s = screen(&a);
    assert!(s.contains("raw records"), "{s}");
    assert!(s.contains("ttl 1/120"), "superseded address flushed: {s}");
    assert!(s.contains("SRV"), "{s}");
    assert!(s.contains("prio 0 weight 0 port 631 target brother.local"), "{s}");
    assert!(s.contains("ttl 110/120"), "{s}");
    assert!(s.contains("cache-flush"), "{s}");
    assert!(s.contains("src 192.168.1.40  if wlan0"), "{s}");
    assert!(s.contains("via wire"), "{s}");
    assert!(s.contains("first "), "{s}");
    assert!(s.contains("\"bin=0x0102\""), "non-UTF-8 TXT shown as hex: {s}");
}

#[tokio::test(start_paused = true)]
async fn raw_view_keeps_expired_records() {
    let store = lan();
    advance(Duration::from_secs(121)).await;
    let mut a = app(&store);
    keys(&mut a, "jjld");
    let s = screen(&a);
    assert!(s.contains("[expired]"), "{s}");
}

#[tokio::test(start_paused = true)]
async fn retained_device_rendered_as_gone_then_removed() {
    let mut store = lan();
    let mut a = app(&store);
    key(&mut a, KeyCode::Char('j')); // nas.local
    store.set_retained(a.retained_ids());
    advance(Duration::from_secs(400)).await;
    address(&mut store, "brother.local", 40);
    address(&mut store, "tv.local", 20);
    store.gc(Instant::now());
    a.set_snapshot(Arc::new(store.snapshot(Instant::now())));
    let s = screen(&a);
    let nas = s.lines().find(|l| l.contains("NAS")).unwrap();
    assert!(nas.contains("gone 6m"), "{nas}");
    assert!(s.contains("(pinned)"), "{s}");

    key(&mut a, KeyCode::Char('k')); // move away
    store.set_retained(a.retained_ids());
    store.gc(Instant::now());
    a.set_snapshot(Arc::new(store.snapshot(Instant::now())));
    assert!(!left_labels(&a).contains(&"nas.local".to_string()));
}

#[tokio::test(start_paused = true)]
async fn retained_device_returns_in_place() {
    let mut store = lan();
    let mut a = app(&store);
    key(&mut a, KeyCode::Char('j')); // nas.local
    store.set_retained(a.retained_ids());
    advance(Duration::from_secs(400)).await;
    store.gc(Instant::now());
    address(&mut store, "nas.local", 10);
    a.set_snapshot(Arc::new(store.snapshot(Instant::now())));
    assert_eq!(a.left.as_deref(), Some("nas.local"));
    let d = a.selected_device().unwrap();
    assert!(!d.gone);
}

#[tokio::test(start_paused = true)]
async fn status_bar_sources() {
    let mut store = lan();
    let s = screen(&app(&store));
    assert!(s.contains("avahi + wire │ 1 interface │ 3 devices"), "{s}");

    store.ingest(Observation::SourceStatus {
        source: Source::System,
        state: SourceState::Unavailable("no daemon".into()),
    });
    assert!(screen(&app(&store)).contains("wire only"));

    store.ingest(Observation::SourceStatus {
        source: Source::System,
        state: SourceState::Active,
    });
    store.ingest(Observation::SourceStatus {
        source: Source::Wire,
        state: SourceState::Unavailable("bind".into()),
    });
    assert!(screen(&app(&store)).contains("system only"));
}
