//! Network interface discovery and selection.

use std::net::{Ipv4Addr, Ipv6Addr};

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::config::InterfaceConfig;

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
    fn default_selection_on_macos() {
        let mut lo0 = iface("lo0", 1);
        lo0.loopback = true;
        let mac = ["en0", "awdl0", "llw0", "utun3", "anpi0"]
            .into_iter()
            .zip(2..)
            .map(|(name, index)| iface(name, index));
        let all: Vec<Interface> = std::iter::once(lo0).chain(mac).collect();
        let selected = Selector::new(&InterfaceConfig::default()).unwrap().select(&all);
        let names: Vec<_> = selected.into_iter().map(|i| i.name).collect();
        assert_eq!(names, vec!["en0"]);
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
