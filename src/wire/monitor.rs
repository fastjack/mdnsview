//! The interface monitor: decides which interfaces discovery uses, from the
//! configured rules and the user's runtime selection.

use std::io;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::MissedTickBehavior;
use tracing::warn;

use super::interfaces::{Interface, Selector};
use super::sys;
use crate::model::record::{InterfaceSelection, Observation};

const INTERFACE_POLL: Duration = Duration::from_secs(5);

/// Lists the host's interfaces; replaceable in tests.
pub type Lister = Box<dyn Fn() -> io::Result<Vec<Interface>> + Send>;

/// The interfaces discovery may use, and the ones it uses now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InterfaceSet {
    /// Interfaces passing the configured rules, sorted by name.
    pub eligible: Vec<Interface>,
    /// The eligible interfaces kept by the runtime selection.
    pub active: Vec<Interface>,
}

pub struct Monitor {
    selector: Selector,
    lister: Lister,
    selection: watch::Receiver<InterfaceSelection>,
    current: InterfaceSet,
}

impl Monitor {
    pub fn new(selector: Selector, selection: watch::Receiver<InterfaceSelection>) -> Self {
        Self::with_lister(selector, Box::new(sys::list_interfaces), selection)
    }

    pub fn with_lister(
        selector: Selector,
        lister: Lister,
        mut selection: watch::Receiver<InterfaceSelection>,
    ) -> Self {
        selection.mark_unchanged();
        let mut monitor = Self {
            selector,
            lister,
            selection,
            current: InterfaceSet::default(),
        };
        monitor.current = monitor.compute();
        monitor
    }

    /// The interface set of the default configuration on this host, for
    /// live tests.
    #[cfg(test)]
    pub fn default_set() -> InterfaceSet {
        let (_selection, selection) = watch::channel(InterfaceSelection::All);
        let selector = Selector::new(&crate::config::InterfaceConfig::default()).unwrap();
        Self::new(selector, selection).initial()
    }

    /// The interface set at startup, known before any backend runs.
    pub fn initial(&self) -> InterfaceSet {
        self.current.clone()
    }

    fn compute(&self) -> InterfaceSet {
        let eligible = match (self.lister)() {
            Ok(all) => self.selector.select(&all),
            Err(e) => {
                warn!("cannot list interfaces: {e}");
                self.current.eligible.clone()
            }
        };
        let active = Selector::active(&eligible, &self.selection.borrow());
        InterfaceSet { eligible, active }
    }

    fn observation(&self) -> Observation {
        Observation::Interfaces {
            eligible: self.current.eligible.iter().map(Interface::info).collect(),
            active: self.current.active.iter().map(|i| i.name.clone()).collect(),
            selection: self.selection.borrow().clone(),
        }
    }

    /// Publishes the interface set on `sets` and reports it to the store,
    /// re-evaluating on every poll and on every selection change.
    pub async fn run(mut self, sets: watch::Sender<InterfaceSet>, observations: mpsc::Sender<Observation>) {
        sets.send_replace(self.current.clone());
        if observations.send(self.observation()).await.is_err() {
            return;
        }
        let mut poll = tokio::time::interval(INTERFACE_POLL);
        poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
        poll.tick().await;
        let mut selection_open = true;
        loop {
            let selection_changed = tokio::select! {
                _ = poll.tick() => false,
                changed = self.selection.changed(), if selection_open => {
                    selection_open = changed.is_ok();
                    selection_open
                }
            };
            let next = self.compute();
            if next == self.current && !selection_changed {
                continue;
            }
            if next != self.current {
                self.current = next;
                sets.send_replace(self.current.clone());
            }
            if observations.send(self.observation()).await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use tokio::time::timeout;

    use super::*;
    use crate::config::InterfaceConfig;

    fn iface(name: &str, index: u32) -> Interface {
        Interface {
            name: name.into(),
            index,
            up: true,
            multicast: true,
            loopback: false,
            v4: vec![Ipv4Addr::new(10, 0, 0, index as u8)],
            v6: Vec::new(),
        }
    }

    fn names(list: &[Interface]) -> Vec<&str> {
        list.iter().map(|i| i.name.as_str()).collect()
    }

    fn monitor(selection: watch::Receiver<InterfaceSelection>) -> Monitor {
        let selector = Selector::new(&InterfaceConfig::default()).unwrap();
        let lister: Lister = Box::new(|| Ok(vec![iface("wlan0", 2), iface("enp3s0", 3), iface("veth1a2b", 4)]));
        Monitor::with_lister(selector, lister, selection)
    }

    #[test]
    fn initial_set_applies_rules_and_selection() {
        let (_tx, rx) = watch::channel(InterfaceSelection::All);
        let set = monitor(rx).initial();
        assert_eq!(names(&set.eligible), vec!["enp3s0", "wlan0"]);
        assert_eq!(names(&set.active), vec!["enp3s0", "wlan0"]);
    }

    #[tokio::test]
    async fn selection_change_applies_without_waiting_for_poll() {
        let (sel_tx, sel_rx) = watch::channel(InterfaceSelection::All);
        let (sets_tx, mut sets_rx) = watch::channel(InterfaceSet::default());
        let (obs_tx, mut obs_rx) = mpsc::channel(16);
        let task = tokio::spawn(monitor(sel_rx).run(sets_tx, obs_tx));

        let first = obs_rx.recv().await.unwrap();
        assert!(matches!(first, Observation::Interfaces { ref active, .. } if active.len() == 2));
        sets_rx.mark_unchanged();

        sel_tx.send_replace(InterfaceSelection::Only(["enp3s0".to_string()].into()));
        timeout(Duration::from_secs(1), sets_rx.changed()).await.unwrap().unwrap();
        assert_eq!(names(&sets_rx.borrow().active), vec!["enp3s0"]);
        assert_eq!(names(&sets_rx.borrow().eligible), vec!["enp3s0", "wlan0"]);

        let obs = timeout(Duration::from_secs(1), obs_rx.recv()).await.unwrap().unwrap();
        let Observation::Interfaces { eligible, active, selection } = obs else {
            panic!("expected interfaces, got {obs:?}");
        };
        assert_eq!(eligible.len(), 2);
        assert_eq!(active, vec!["enp3s0"]);
        assert_eq!(selection, InterfaceSelection::Only(["enp3s0".to_string()].into()));
        task.abort();
    }
}
