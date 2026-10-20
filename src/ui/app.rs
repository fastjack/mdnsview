//! UI state and key handling, independent of rendering.

use std::sync::Arc;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::model::snapshot::{Device, Service, ServiceType, Snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Devices,
    Types,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailMode {
    Simple,
    Raw,
}

/// Where the last frame put the panes, for mouse hit-testing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HitMap {
    pub left: Rect,
    pub middle: Rect,
    pub right: Rect,
    /// Row areas of the two lists (inside their borders).
    pub left_rows: Rect,
    pub middle_rows: Rect,
    /// Index of the first visible entry of each list.
    pub left_offset: usize,
    pub middle_offset: usize,
}

impl HitMap {
    fn pane_at(&self, pos: Position) -> Option<Focus> {
        [(self.left, Focus::Left), (self.middle, Focus::Middle), (self.right, Focus::Right)]
            .into_iter()
            .find(|(area, _)| area.contains(pos))
            .map(|(_, pane)| pane)
    }

    /// The entry index under `pos` in a list pane, if `pos` is on a row.
    fn row_at(&self, pane: Focus, pos: Position) -> Option<usize> {
        let (rows, offset) = match pane {
            Focus::Left => (self.left_rows, self.left_offset),
            Focus::Middle => (self.middle_rows, self.middle_offset),
            Focus::Right => return None,
        };
        rows.contains(pos)
            .then(|| offset + usize::from(pos.y - rows.y))
    }
}

/// An entry of the left pane.
pub enum LeftItem<'a> {
    Device(&'a Device),
    Type(&'a ServiceType),
}

impl LeftItem<'_> {
    pub fn id(&self) -> &str {
        match self {
            LeftItem::Device(d) => &d.id,
            LeftItem::Type(t) => &t.id,
        }
    }
}

pub struct App {
    pub snapshot: Arc<Snapshot>,
    pub mode: Mode,
    pub focus: Focus,
    pub detail: DetailMode,
    pub filter: String,
    pub editing_filter: bool,
    pub left: Option<String>,
    pub middle: Option<String>,
    /// Instance and host key of the selected service, to find it again when
    /// its id changes because another host announces the same instance name.
    middle_origin: Option<(String, Option<String>)>,
    pub detail_scroll: u16,
    pub quit: bool,
    /// Whether mouse capture should be on.
    pub mouse: bool,
}

impl App {
    pub fn new(snapshot: Arc<Snapshot>) -> Self {
        let mut app = Self {
            snapshot,
            mode: Mode::Devices,
            focus: Focus::Left,
            detail: DetailMode::Simple,
            filter: String::new(),
            editing_filter: false,
            left: None,
            middle: None,
            middle_origin: None,
            detail_scroll: 0,
            quit: false,
            mouse: true,
        };
        app.fix_selection();
        app
    }

    pub fn set_snapshot(&mut self, snapshot: Arc<Snapshot>) {
        self.snapshot = snapshot;
        self.fix_selection();
    }

    /// Entries are hidden once past the timeout, unless retained.
    fn visible(gone: bool, retained: bool) -> bool {
        !gone || retained
    }

    fn matches(&self, text: &str) -> bool {
        text.to_lowercase().contains(&self.filter.to_lowercase())
    }

    fn device_matches(&self, d: &Device) -> bool {
        self.filter.is_empty()
            || self.matches(&d.label)
            || d.hosts.iter().any(|h| self.matches(&h.to_string()))
            || d.services.iter().any(|s| {
                let s = &self.snapshot.services[s];
                self.matches(&s.name) || self.matches(&s.service_type) || self.matches(&s.type_label)
            })
    }

    fn type_matches(&self, t: &ServiceType) -> bool {
        self.filter.is_empty()
            || self.matches(&t.id)
            || self.matches(&t.label)
            || t.instances.iter().any(|s| {
                let s = &self.snapshot.services[s];
                self.matches(&s.name) || s.host.as_ref().is_some_and(|h| self.matches(&h.to_string()))
            })
    }

    pub fn left_items(&self) -> Vec<LeftItem<'_>> {
        match self.mode {
            Mode::Devices => self
                .snapshot
                .devices
                .iter()
                .filter(|d| Self::visible(d.gone, d.retained) && self.device_matches(d))
                .map(LeftItem::Device)
                .collect(),
            Mode::Types => self
                .snapshot
                .types
                .iter()
                .filter(|t| self.type_matches(t) && !self.middle_ids_of(&t.instances).is_empty())
                .map(LeftItem::Type)
                .collect(),
        }
    }

    fn middle_ids_of(&self, ids: &[String]) -> Vec<String> {
        ids.iter()
            .filter(|id| {
                let s = &self.snapshot.services[*id];
                Self::visible(s.gone, s.retained)
            })
            .cloned()
            .collect()
    }

    pub fn middle_items(&self) -> Vec<&Service> {
        let Some(left) = &self.left else {
            return Vec::new();
        };
        let ids = match self.mode {
            Mode::Devices => self.snapshot.device(left).map(|d| &d.services),
            Mode::Types => self.snapshot.service_type(left).map(|t| &t.instances),
        };
        ids.map(|ids| self.middle_ids_of(ids))
            .unwrap_or_default()
            .iter()
            .map(|id| &self.snapshot.services[id])
            .collect()
    }

    pub fn selected_device(&self) -> Option<&Device> {
        match self.mode {
            Mode::Devices => self.left.as_deref().and_then(|id| self.snapshot.device(id)),
            Mode::Types => self
                .selected_service()
                .and_then(|s| self.snapshot.device(&s.device_id)),
        }
    }

    pub fn selected_service(&self) -> Option<&Service> {
        self.middle.as_deref().and_then(|id| self.snapshot.services.get(id))
    }

    /// Keeps selections pointing at visible entries; falls back to the first one.
    fn fix_selection(&mut self) {
        let left_ids: Vec<String> = self.left_items().iter().map(|i| i.id().to_string()).collect();
        // A device id is a host key: after a merge, follow that host to its device.
        if let Some(left) = &self.left
            && !left_ids.contains(left)
            && let Some(merged) = self.left_items().iter().find_map(|i| match i {
                LeftItem::Device(d) if d.hosts.iter().any(|h| h.key() == left) => Some(d.id.clone()),
                _ => None,
            })
        {
            self.left = Some(merged);
        }
        if !self.left.as_ref().is_some_and(|l| left_ids.contains(l)) {
            self.left = left_ids.first().cloned();
            self.middle = None;
            self.detail_scroll = 0;
        }
        let middle: Vec<(String, (String, Option<String>))> =
            self.middle_items().iter().map(|s| (s.id.clone(), Self::origin(s))).collect();
        if !self.middle.as_ref().is_some_and(|m| middle.iter().any(|(id, _)| id == m)) {
            let same = self
                .middle_origin
                .as_ref()
                .filter(|_| self.middle.is_some())
                .and_then(|origin| middle.iter().find(|(_, o)| o == origin))
                .map(|(id, _)| id.clone());
            if same.is_none() {
                self.detail_scroll = 0;
            }
            self.middle = same.or_else(|| middle.first().map(|(id, _)| id.clone()));
        }
        self.middle_origin = self.selected_service().map(Self::origin);
    }

    fn origin(s: &Service) -> (String, Option<String>) {
        (s.instance.key().to_string(), s.host.as_ref().map(|h| h.key().to_string()))
    }

    /// Ids the store must keep while they are selected.
    pub fn retained_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.left.iter().cloned().collect();
        if let Some(s) = self.selected_service() {
            ids.push(s.id.clone());
            if !ids.contains(&s.device_id) {
                ids.push(s.device_id.clone());
            }
        }
        ids
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.editing_filter {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.editing_filter = false;
                }
                KeyCode::Enter => self.editing_filter = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) => self.filter.push(c),
                _ => {}
            }
            self.fix_selection();
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('m') => self.mouse = !self.mouse,
            KeyCode::Char('/') => self.editing_filter = true,
            KeyCode::Esc => {
                self.filter.clear();
                self.fix_selection();
            }
            KeyCode::Tab | KeyCode::BackTab => self.pivot(),
            KeyCode::Char('d') => {
                self.detail = match self.detail {
                    DetailMode::Simple => DetailMode::Raw,
                    DetailMode::Raw => DetailMode::Simple,
                };
                self.detail_scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_in(self.focus, -1),
            KeyCode::Down | KeyCode::Char('j') => self.move_in(self.focus, 1),
            KeyCode::PageUp => self.move_in(self.focus, -10),
            KeyCode::PageDown => self.move_in(self.focus, 10),
            KeyCode::Left | KeyCode::Char('h') => {
                self.focus = match self.focus {
                    Focus::Right => Focus::Middle,
                    _ => Focus::Left,
                }
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                self.focus = match self.focus {
                    Focus::Left => Focus::Middle,
                    _ => Focus::Right,
                }
            }
            _ => {}
        }
    }

    /// Mouse clicks select and focus; the wheel acts on the pane under the pointer.
    pub fn on_mouse(&mut self, event: MouseEvent, hits: &HitMap) {
        let pos = Position::new(event.column, event.row);
        let Some(pane) = hits.pane_at(pos) else {
            return;
        };
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = pane;
                if let Some(index) = hits.row_at(pane, pos) {
                    self.select_index(pane, index);
                }
            }
            MouseEventKind::ScrollUp => self.move_in(pane, -1),
            MouseEventKind::ScrollDown => self.move_in(pane, 1),
            _ => {}
        }
    }

    fn pane_ids(&self, pane: Focus) -> Vec<String> {
        match pane {
            Focus::Left => self.left_items().iter().map(|i| i.id().to_string()).collect(),
            Focus::Middle => self.middle_items().iter().map(|s| s.id.clone()).collect(),
            Focus::Right => Vec::new(),
        }
    }

    fn select(&mut self, pane: Focus, id: String) {
        match pane {
            Focus::Left => {
                if self.left.as_ref() != Some(&id) {
                    self.left = Some(id);
                    self.middle = None;
                }
            }
            Focus::Middle => self.middle = Some(id),
            Focus::Right => return,
        }
        self.detail_scroll = 0;
        self.fix_selection();
    }

    /// Selects the entry at `index` of a list pane, if there is one.
    fn select_index(&mut self, pane: Focus, index: usize) {
        if let Some(id) = self.pane_ids(pane).into_iter().nth(index) {
            self.select(pane, id);
        }
    }

    /// Moves the selection of a list pane, or scrolls the details pane.
    fn move_in(&mut self, pane: Focus, delta: isize) {
        if pane == Focus::Right {
            let scroll = i64::from(self.detail_scroll) + delta as i64;
            self.detail_scroll = scroll.clamp(0, i64::from(u16::MAX)) as u16;
            return;
        }
        let ids = self.pane_ids(pane);
        let current = match pane {
            Focus::Left => self.left.as_deref(),
            _ => self.middle.as_deref(),
        };
        if let Some(next) = step(&ids, current, delta) {
            self.select(pane, next);
        }
    }

    /// Switches the left pane between devices and service types, keeping the
    /// selected service selected when there is one.
    fn pivot(&mut self) {
        let service = self.selected_service().cloned();
        self.mode = match self.mode {
            Mode::Devices => Mode::Types,
            Mode::Types => Mode::Devices,
        };
        match service {
            Some(s) => {
                self.left = Some(match self.mode {
                    Mode::Devices => s.device_id.clone(),
                    Mode::Types => s.service_type.clone(),
                });
                self.middle = Some(s.id);
            }
            None => {
                self.left = None;
                self.middle = None;
            }
        }
        self.detail_scroll = 0;
        self.fix_selection();
    }
}

fn step(ids: &[String], current: Option<&str>, delta: isize) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let pos = current.and_then(|c| ids.iter().position(|i| i == c)).unwrap_or(0) as isize;
    let next = (pos + delta).clamp(0, ids.len() as isize - 1) as usize;
    Some(ids[next].clone())
}
