//! Rendering of the three panes and the status bar.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Paragraph, Wrap};
use tokio::time::Instant;

use super::app::{App, DetailMode, Focus, HitMap, LeftItem, Mode};
use crate::model::record::{RData, rtype};
use crate::model::snapshot::{Device, Liveness, RecordView, Service, txt_entries};

const DIM: Style = Style::new().fg(Color::DarkGray);
const KEY: Style = Style::new().fg(Color::Cyan);

/// Draws the UI and returns where the panes and list rows ended up.
pub fn render(frame: &mut Frame, app: &App, system_name: &str) -> HitMap {
    let [main, status] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(frame.area());
    let [left, middle, right] = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(30),
        Constraint::Percentage(40),
    ])
    .areas(main);
    let now = Instant::now();
    let (left_rows, left_offset) = render_left(frame, app, left, now);
    let (middle_rows, middle_offset) = render_middle(frame, app, middle, now);
    render_details(frame, app, right, now);
    render_status(frame, app, status, system_name);
    HitMap {
        left,
        middle,
        right,
        left_rows,
        middle_rows,
        left_offset,
        middle_offset,
    }
}

fn block(title: &str, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new()
    };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style)
        .title(format!(" {title} "))
}

/// `now`, `14s`, `3m`, `2h`
pub fn relative(age: Duration) -> String {
    let s = age.as_secs();
    match s {
        0..2 => "now".to_string(),
        2..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}

fn row_style(state: Liveness, gone: bool) -> Style {
    if gone || state != Liveness::Live {
        DIM
    } else {
        Style::new()
    }
}

/// Label left-aligned, `right` right-aligned within `width` columns.
fn two_columns(label: &str, right: &str, width: usize) -> String {
    let right_w = right.chars().count();
    let label_w = width.saturating_sub(right_w + 1);
    let mut label: String = label.chars().take(label_w).collect();
    let pad = label_w.saturating_sub(label.chars().count());
    label.push_str(&" ".repeat(pad));
    format!("{label} {right}")
}

fn age_text(gone: bool, state: Liveness, age: Duration) -> String {
    let rel = relative(age);
    if gone {
        format!("gone {rel}")
    } else if state == Liveness::Left {
        format!("left {rel}")
    } else {
        rel
    }
}

/// Returns the rows area and the scroll offset ratatui chose.
fn render_list(
    frame: &mut Frame,
    area: Rect,
    block: Block<'_>,
    items: Vec<ListItem<'_>>,
    selected: Option<usize>,
    focused: bool,
) -> (Rect, usize) {
    let highlight = if focused {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new().add_modifier(Modifier::BOLD).bg(Color::DarkGray)
    };
    let empty = items.is_empty();
    let rows = block.inner(area);
    let list = List::new(items).block(block).highlight_style(highlight);
    let mut state = ListState::default().with_selected(if empty { None } else { selected });
    frame.render_stateful_widget(list, area, &mut state);
    (rows, state.offset())
}

fn render_left(frame: &mut Frame, app: &App, area: Rect, now: Instant) -> (Rect, usize) {
    let width = area.width.saturating_sub(2) as usize;
    let items = app.left_items();
    let selected = items.iter().position(|i| Some(i.id()) == app.left.as_deref());
    let rows: Vec<ListItem> = items
        .iter()
        .map(|item| match item {
            LeftItem::Device(d) => {
                let age = age_text(d.gone, d.state, now.saturating_duration_since(d.last_seen));
                ListItem::new(two_columns(&d.label, &age, width)).style(row_style(d.state, d.gone))
            }
            LeftItem::Type(t) => {
                ListItem::new(two_columns(&t.label, &t.instances.len().to_string(), width))
                    .style(row_style(t.state, false))
            }
        })
        .collect();
    let title = match app.mode {
        Mode::Devices => "Devices",
        Mode::Types => "Service types",
    };
    render_list(frame, area, block(title, app.focus == Focus::Left), rows, selected, app.focus == Focus::Left)
}

fn render_middle(frame: &mut Frame, app: &App, area: Rect, now: Instant) -> (Rect, usize) {
    let width = area.width.saturating_sub(2) as usize;
    let items = app.middle_items();
    let selected = items.iter().position(|s| Some(&s.id) == app.middle.as_ref());
    let device_label = app.selected_device().map(|d| d.label.clone()).unwrap_or_default();
    let rows: Vec<ListItem> = items
        .iter()
        .map(|s| {
            let label = match app.mode {
                Mode::Devices if s.name == device_label => s.type_label.clone(),
                Mode::Devices => format!("{} · {}", s.type_label, s.name),
                Mode::Types => format!(
                    "{} @{}",
                    s.name,
                    s.host.as_ref().map(|h| h.to_string()).unwrap_or_else(|| "?".into())
                ),
            };
            let age = if s.gone || s.state != Liveness::Live {
                age_text(s.gone, s.state, now.saturating_duration_since(s.last_seen))
            } else {
                String::new()
            };
            ListItem::new(two_columns(&label, &age, width)).style(row_style(s.state, s.gone))
        })
        .collect();
    let title = match app.mode {
        Mode::Devices => "Services",
        Mode::Types => "Instances",
    };
    render_list(frame, area, block(title, app.focus == Focus::Middle), rows, selected, app.focus == Focus::Middle)
}

fn field(name: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{name:<9} "), DIM),
        Span::raw(value.into()),
    ])
}

fn state_line(state: Liveness, gone: bool, retained: bool, age: Duration) -> Line<'static> {
    let text = match (state, gone) {
        (_, true) if retained => format!("gone, last seen {} ago (pinned)", relative(age)),
        (_, true) => format!("gone, last seen {} ago", relative(age)),
        (Liveness::Live, _) if age < Duration::from_secs(2) => "live, seen just now".to_string(),
        (Liveness::Live, _) => format!("live, last seen {} ago", relative(age)),
        (Liveness::Stale, _) => format!("stale, last seen {} ago", relative(age)),
        (Liveness::Left, _) => format!("left, last seen {} ago", relative(age)),
    };
    field("State", text)
}

fn render_details(frame: &mut Frame, app: &App, area: Rect, now: Instant) {
    let title = match app.detail {
        DetailMode::Simple => "Details",
        DetailMode::Raw => "Details · raw records",
    };
    let lines = match (app.selected_service(), app.selected_device()) {
        (Some(s), _) => match app.detail {
            DetailMode::Simple => simple_service(s, app.snapshot.device(&s.device_id), now),
            DetailMode::Raw => raw_records(&s.records),
        },
        (None, Some(d)) => device_details(d, now),
        (None, None) => vec![Line::styled("Nothing selected.", DIM)],
    };
    let paragraph = Paragraph::new(lines)
        .block(block(title, app.focus == Focus::Right))
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
    frame.render_widget(paragraph, area);
}

fn simple_service(s: &Service, device: Option<&Device>, now: Instant) -> Vec<Line<'static>> {
    let mut lines = vec![
        field("Name", s.name.clone()),
        field("Type", format!("{} ({})", s.type_label, s.service_type)),
        field(
            "Host",
            s.host.as_ref().map(|h| h.to_string()).unwrap_or_else(|| "(not resolved)".into()),
        ),
    ];
    if let Some(d) = device.filter(|d| d.hosts.len() > 1) {
        let hosts: Vec<String> = d.hosts.iter().map(|h| h.to_string()).collect();
        lines.push(field("Device", hosts.join(", ")));
    }
    if !s.shared_with.is_empty() {
        let hosts: Vec<String> = s.shared_with.iter().map(|h| h.to_string()).collect();
        lines.push(field("Shared name", hosts.join(", ")));
    }
    if s.addresses.is_empty() {
        lines.push(field("Addresses", "-"));
    }
    for (i, a) in s.addresses.iter().enumerate() {
        lines.push(field(if i == 0 { "Addresses" } else { "" }, a.to_string()));
    }
    lines.push(field("Port", s.port.map(|p| p.to_string()).unwrap_or_else(|| "-".into())));
    lines.push(state_line(s.state, s.gone, s.retained, now.saturating_duration_since(s.last_seen)));
    lines.push(Line::raw(""));
    if s.txt.is_empty() {
        lines.push(Line::styled("No TXT data.", DIM));
    } else {
        lines.push(Line::styled("TXT", DIM));
        let key_w = s.txt.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0).min(20);
        for (k, v) in &s.txt {
            let value = v.as_deref().map(txt_value).unwrap_or_default();
            lines.push(Line::raw(format!("  {k:<key_w$}  {value}")));
        }
    }
    lines
}

fn device_details(d: &Device, now: Instant) -> Vec<Line<'static>> {
    let mut lines = vec![field("Device", d.label.clone())];
    if d.hosts.is_empty() {
        lines.push(field("Hosts", "(not resolved)"));
    }
    for (i, h) in d.hosts.iter().enumerate() {
        lines.push(field(if i == 0 { "Hosts" } else { "" }, h.to_string()));
    }
    for (i, a) in d.addresses.iter().enumerate() {
        lines.push(field(if i == 0 { "Addresses" } else { "" }, a.to_string()));
    }
    lines.push(field("Services", d.services.len().to_string()));
    lines.push(state_line(d.state, d.gone, d.retained, now.saturating_duration_since(d.last_seen)));
    lines
}

/// TXT value as text, or hexadecimal when it is not valid UTF-8.
pub fn txt_value(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) if !s.chars().any(|c| c.is_control()) => s.to_string(),
        _ => format!("0x{}", hex(bytes)),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn rdata_text(data: &RData) -> String {
    match data {
        RData::A(a) => a.to_string(),
        RData::Aaaa(a) => a.to_string(),
        RData::Ptr(n) => n.to_string(),
        RData::Srv { priority, weight, port, target } => {
            format!("prio {priority} weight {weight} port {port} target {target}")
        }
        RData::Txt(strings) => {
            if strings.iter().all(|s| s.is_empty()) {
                return "(empty)".to_string();
            }
            txt_entries(strings)
                .into_iter()
                .map(|(k, v)| match v {
                    Some(v) => format!("\"{k}={}\"", txt_value(&v)),
                    None => format!("\"{k}\""),
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        RData::Hinfo { cpu, os } => format!("cpu \"{cpu}\" os \"{os}\""),
        RData::Nsec { next, types } => format!(
            "next {next} types {}",
            types.iter().map(|t| rtype::name(*t)).collect::<Vec<_>>().join(" ")
        ),
        RData::Unknown { data, .. } => format!("\\# {} {}", data.len(), hex(data)),
        RData::Other { text, .. } => text.clone(),
    }
}

fn raw_records(records: &[RecordView]) -> Vec<Line<'static>> {
    if records.is_empty() {
        return vec![Line::styled("No records.", DIM)];
    }
    let mut lines = Vec::new();
    for r in records {
        let style = if r.expired { DIM } else { Style::new() };
        let mut head = vec![
            Span::styled(format!("{:<5} ", rtype::name(r.data.rtype())), style.add_modifier(Modifier::BOLD)),
            Span::styled(r.name.to_string(), style),
        ];
        if r.goodbye {
            head.push(Span::styled("  [goodbye]", Style::new().fg(Color::Yellow)));
        } else if r.expired {
            head.push(Span::styled("  [expired]", Style::new().fg(Color::Yellow)));
        }
        lines.push(Line::from(head));
        lines.push(Line::styled(format!("      {}", rdata_text(&r.data)), style));
        let ttl = match (r.remaining, r.ttl) {
            (Some(rem), Some(ttl)) => format!("ttl {}/{ttl}", rem.as_secs()),
            _ => "ttl -".to_string(),
        };
        let flush = if r.cache_flush { "cache-flush" } else { "shared" };
        lines.push(Line::styled(
            format!("      {ttl}  {flush}  via {}", r.sources.label()),
            DIM,
        ));
        lines.push(Line::styled(
            format!(
                "      src {}  if {}",
                r.src_addr.map(|a| a.to_string()).unwrap_or_else(|| "-".into()),
                r.iface.as_deref().unwrap_or("-")
            ),
            DIM,
        ));
        lines.push(Line::styled(
            format!("      first {}  last {}", clock(r.first_seen), clock(r.last_seen)),
            DIM,
        ));
    }
    lines
}

/// Local wall-clock time as HH:MM:SS.
pub fn clock(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as libc::time_t;
    // SAFETY: `localtime_r` writes only into the provided `tm`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    if ok {
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    } else {
        "--:--:--".to_string()
    }
}

fn render_status(frame: &mut Frame, app: &App, area: Rect, system_name: &str) {
    let mut spans = Vec::new();
    if app.editing_filter || !app.filter.is_empty() {
        spans.push(Span::styled("/", KEY));
        spans.push(Span::raw(app.filter.clone()));
        if app.editing_filter {
            spans.push(Span::styled("▏", KEY));
        }
        spans.push(Span::styled(" │ ", DIM));
    }
    let status = &app.snapshot.status;
    let devices = match app.mode {
        Mode::Devices => app.left_items().len(),
        Mode::Types => app
            .snapshot
            .devices
            .iter()
            .filter(|d| !d.gone || d.retained)
            .count(),
    };
    spans.push(Span::raw(status.sources_label(system_name)));
    spans.push(Span::styled(" │ ", DIM));
    spans.push(Span::raw(format!(
        "{} interface{}",
        status.interfaces.len(),
        if status.interfaces.len() == 1 { "" } else { "s" }
    )));
    spans.push(Span::styled(" │ ", DIM));
    spans.push(Span::raw(format!("{devices} device{}", if devices == 1 { "" } else { "s" })));
    spans.push(Span::styled(" │ ", DIM));
    spans.push(Span::raw(if app.mouse { "mouse on" } else { "mouse off" }));
    spans.push(Span::styled(" │ ", DIM));
    let hints: &[(&str, &str)] = if app.editing_filter {
        &[("enter", "apply"), ("esc", "clear")]
    } else {
        &[("tab", "pivot"), ("/", "filter"), ("d", "details"), ("m", "mouse"), ("q", "quit")]
    };
    for (i, (key, what)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(*key, KEY));
        spans.push(Span::styled(format!(" {what}"), DIM));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
