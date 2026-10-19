//! UI state and key handling, independent of rendering.

use std::sync::Arc;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

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
    pub detail_scroll: u16,
    pub quit: bool,
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
            detail_scroll: 0,
            quit: false,
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
            || d.host.as_ref().is_some_and(|h| self.matches(&h.to_string()))
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
        if !self.left.as_ref().is_some_and(|l| left_ids.contains(l)) {
            self.left = left_ids.first().cloned();
            self.middle = None;
            self.detail_scroll = 0;
        }
        let middle_ids: Vec<String> = self.middle_items().iter().map(|s| s.id.clone()).collect();
        if !self.middle.as_ref().is_some_and(|m| middle_ids.contains(m)) {
            self.middle = middle_ids.first().cloned();
            self.detail_scroll = 0;
        }
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
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
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

    fn move_selection(&mut self, delta: isize) {
        match self.focus {
            Focus::Left => {
                let ids: Vec<String> = self.left_items().iter().map(|i| i.id().to_string()).collect();
                if let Some(next) = step(&ids, self.left.as_deref(), delta)
                    && self.left.as_ref() != Some(&next)
                {
                    self.left = Some(next);
                    self.middle = None;
                }
            }
            Focus::Middle => {
                let ids: Vec<String> = self.middle_items().iter().map(|s| s.id.clone()).collect();
                if let Some(next) = step(&ids, self.middle.as_deref(), delta) {
                    self.middle = Some(next);
                }
            }
            Focus::Right => {
                let scroll = i64::from(self.detail_scroll) + delta as i64;
                self.detail_scroll = scroll.clamp(0, i64::from(u16::MAX)) as u16;
                return;
            }
        }
        self.detail_scroll = 0;
        self.fix_selection();
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
