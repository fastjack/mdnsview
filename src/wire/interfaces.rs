//! Network interface discovery and selection.

use std::net::{Ipv4Addr, Ipv6Addr};

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::config::InterfaceConfig;
use crate::model::record::{InterfaceInfo, InterfaceSelection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub index: u32,
    pub up: bool,
    pub multicast: bool,
    pub loopback: bool,
    pub v4: Vec<Ipv4Addr>,
    pub v6: Vec<Ipv6Addr>,
}

/// Applies the configured include / exclude patterns to interface lists.
pub struct Selector {
    include: GlobSet,
    exclude: GlobSet,
}

impl Selector {
    pub fn new(config: &InterfaceConfig) -> Result<Self> {
        Ok(Self {
            include: globs(&config.include)?,
            exclude: globs(&config.exclude)?,
        })
    }

    /// Interfaces to use, sorted by name: up, multicast-capable and not
    /// loopback, then filtered by include (when non-empty) and exclude.
    pub fn select(&self, all: &[Interface]) -> Vec<Interface> {
        let mut selected: Vec<Interface> = all
            .iter()
            .filter(|i| i.up && i.multicast && !i.loopback)
            .filter(|i| self.include.is_empty() || self.include.is_match(&i.name))
            .filter(|i| !self.exclude.is_match(&i.name))
            .cloned()
            .collect();
        selected.sort_by(|a, b| a.name.cmp(&b.name));
        selected
    }

    /// The eligible interfaces the runtime selection keeps. Never adds an
    /// interface that is not eligible.
    pub fn active(eligible: &[Interface], selection: &InterfaceSelection) -> Vec<Interface> {
        eligible
            .iter()
            .filter(|i| selection.includes(&i.name))
            .cloned()
            .collect()
    }
}

impl Interface {
    pub fn info(&self) -> InterfaceInfo {
        InterfaceInfo {
            name: self.name.clone(),
            addresses: self
                .v4
                .iter()
                .map(|a| (*a).into())
                .chain(self.v6.iter().map(|a| (*a).into()))
                .collect(),
        }
    }
}

fn globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        builder.add(Glob::new(p)?);
    }
    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn host() -> Vec<Interface> {
        let mut lo = iface("lo", 1);
        lo.loopback = true;
        lo.multicast = false;
        vec![lo, iface("wlan0", 2), iface("docker0", 3), iface("veth1a2b", 4)]
    }

    fn names(config: InterfaceConfig) -> Vec<String> {
        Selector::new(&config)
            .unwrap()
            .select(&host())
            .into_iter()
            .map(|i| i.name)
            .collect()
    }

    #[test]
    fn default_selection() {
        assert_eq!(names(InterfaceConfig::default()), vec!["docker0", "wlan0"]);
    }

    #[test]
    fn include_list() {
        let config = InterfaceConfig {
            include: vec!["wlan0".into()],
            ..Default::default()
        };
        assert_eq!(names(config), vec!["wlan0"]);
    }

    #[test]
    fn include_does_not_bypass_flags() {
        let mut all = host();
        all.push(Interface {
            up: false,
            ..iface("enp3s0", 5)
        });
        let s = Selector::new(&InterfaceConfig {
            include: vec!["enp3s0".into()],
            ..Default::default()
        })
        .unwrap();
        assert!(s.select(&all).is_empty());
    }

    #[test]
    fn exclude_list_replaces_default() {
        let config = InterfaceConfig {
            include: vec![],
            exclude: vec!["docker*".into()],
        };
        assert_eq!(names(config), vec!["veth1a2b", "wlan0"]);
    }

    fn only(names: &[&str]) -> InterfaceSelection {
        InterfaceSelection::Only(names.iter().map(|n| n.to_string()).collect())
    }

    fn active_names(eligible: &[Interface], selection: &InterfaceSelection) -> Vec<String> {
        Selector::active(eligible, selection).into_iter().map(|i| i.name).collect()
    }

    fn eligible() -> Vec<Interface> {
        Selector::new(&InterfaceConfig::default()).unwrap().select(&host())
    }

    #[test]
    fn active_all_keeps_every_eligible_interface() {
        assert_eq!(active_names(&eligible(), &InterfaceSelection::All), vec!["docker0", "wlan0"]);
    }

    #[test]
    fn active_only_keeps_named_interfaces() {
        assert_eq!(active_names(&eligible(), &only(&["wlan0"])), vec!["wlan0"]);
    }

    #[test]
    fn active_skips_absent_selected_interface() {
        assert_eq!(active_names(&eligible(), &only(&["wlan0", "enp3s0"])), vec!["wlan0"]);
    }

    #[test]
    fn new_interface_follows_all_but_not_only() {
        let mut all = host();
        all.push(iface("enp3s0", 5));
        let eligible = Selector::new(&InterfaceConfig::default()).unwrap().select(&all);
        assert_eq!(
            active_names(&eligible, &InterfaceSelection::All),
            vec!["docker0", "enp3s0", "wlan0"]
        );
        assert_eq!(active_names(&eligible, &only(&["wlan0"])), vec!["wlan0"]);
    }

    #[test]
    fn active_never_returns_ineligible_interface() {
        assert!(active_names(&eligible(), &only(&["veth1a2b", "lo"])).is_empty());
    }

    #[test]
    fn non_multicast_interfaces_skipped() {
        let mut all = host();
        all.push(Interface {
            multicast: false,
            ..iface("wg0", 6)
        });
        let s = Selector::new(&InterfaceConfig::default()).unwrap();
        assert!(s.select(&all).iter().all(|i| i.name != "wg0"));
    }
}
