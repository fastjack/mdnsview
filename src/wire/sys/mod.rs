//! Platform-specific socket and interface code.
//!
//! Everything that differs between operating systems lives here: `unix`
//! for Linux and macOS, `windows` for Windows. Both offer the same API.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

pub const MDNS_PORT: u16 = 5353;
pub const GROUP_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub const GROUP_V6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb);

/// A received datagram.
pub struct Received {
    pub len: usize,
    pub src: IpAddr,
    pub ifindex: u32,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::model::record::{DnsName, rtype};
    use crate::wire::codec;

    /// Checks on the real network that group traffic arrives with the index of
    /// the interface it was joined on:
    /// `cargo test live_wire_ifindex -- --ignored --nocapture`.
    /// Set MDNSVIEW_LIVE_IFACE to pick the interface (default en0 on macOS,
    /// WLAN on Windows, eth0 elsewhere).
    #[tokio::test]
    #[ignore = "uses the real network"]
    async fn live_wire_ifindex() {
        let default = if cfg!(target_os = "macos") {
            "en0"
        } else if cfg!(windows) {
            "WLAN"
        } else {
            "eth0"
        };
        let name = std::env::var("MDNSVIEW_LIVE_IFACE").unwrap_or_else(|_| default.into());
        let iface = list_interfaces()
            .unwrap()
            .into_iter()
            .find(|i| i.name == name)
            .unwrap_or_else(|| panic!("no interface {name}"));
        let query = codec::encode_query(
            &[(DnsName::parse("_services._dns-sd._udp.local"), rtype::PTR)],
            &[],
        )
        .unwrap();
        let mut buf = vec![0u8; 9000];
        for socket in [MdnsSocket::open_v4().unwrap(), MdnsSocket::open_v6().unwrap()] {
            let label = if socket.is_v6() { "IPv6" } else { "IPv4" };
            socket.join(&iface).unwrap();
            socket.send_to_group(&query, &iface).unwrap();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            let mut from_others = 0;
            while let Ok(rx) = tokio::time::timeout_at(deadline, socket.recv(&mut buf)).await {
                let rx = rx.unwrap();
                println!("{label}: {} bytes from {} on ifindex {}", rx.len, rx.src, rx.ifindex);
                assert_eq!(rx.ifindex, iface.index, "{label}: wrong interface index");
                let own = match rx.src {
                    IpAddr::V4(a) => iface.v4.contains(&a),
                    IpAddr::V6(a) => iface.v6.contains(&a),
                };
                from_others += usize::from(!own);
            }
            assert!(from_others > 0, "{label}: nothing received from other hosts on {name}");
            socket.leave(&iface).unwrap();
        }
    }
}
