//! Socket and interface code for Unix: Linux and macOS.

use std::collections::BTreeMap;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::AsRawFd;

use nix::ifaddrs::getifaddrs;
use nix::net::if_::{InterfaceFlags, if_nametoindex};
use nix::sys::socket::{ControlMessageOwned, MsgFlags, SockaddrStorage, recvmsg, setsockopt, sockopt};
use socket2::{Domain, InterfaceIndexOrAddress, Protocol, Socket, Type};
use tokio::io::unix::AsyncFd;

use super::{GROUP_V4, GROUP_V6, MDNS_PORT, Received};
use crate::wire::interfaces::Interface;

/// Lists all interfaces with their flags and addresses.
pub fn list_interfaces() -> io::Result<Vec<Interface>> {
    let mut by_name: BTreeMap<String, Interface> = BTreeMap::new();
    for ifa in getifaddrs().map_err(io::Error::from)? {
        let entry = by_name
            .entry(ifa.interface_name.clone())
            .or_insert_with(|| Interface {
                index: if_nametoindex(ifa.interface_name.as_str()).unwrap_or(0),
                name: ifa.interface_name.clone(),
                up: ifa.flags.contains(InterfaceFlags::IFF_UP)
                    && ifa.flags.contains(InterfaceFlags::IFF_RUNNING),
                multicast: ifa.flags.contains(InterfaceFlags::IFF_MULTICAST),
                loopback: ifa.flags.contains(InterfaceFlags::IFF_LOOPBACK),
                v4: Vec::new(),
                v6: Vec::new(),
            });
        if let Some(addr) = ifa.address {
            if let Some(v4) = addr.as_sockaddr_in() {
                entry.v4.push(v4.ip());
            } else if let Some(v6) = addr.as_sockaddr_in6() {
                entry.v6.push(v6.ip());
            }
        }
    }
    Ok(by_name.into_values().filter(|i| i.index != 0).collect())
}

/// One mDNS socket (IPv4 or IPv6) bound to port 5353, shared with other
/// mDNS software on the host.
pub struct MdnsSocket {
    fd: AsyncFd<Socket>,
    v6: bool,
}

impl MdnsSocket {
    pub fn open_v4() -> io::Result<Self> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        share_port(&socket)?;
        socket.set_multicast_ttl_v4(255)?;
        socket.set_multicast_loop_v4(true)?;
        // Only deliver groups joined by this socket, not by others on the host.
        // BSD-derived stacks such as macOS already behave this way.
        #[cfg(target_os = "linux")]
        socket.set_multicast_all_v4(false)?;
        setsockopt(&socket, sockopt::Ipv4PacketInfo, &true).map_err(io::Error::from)?;
        socket.bind(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MDNS_PORT)).into())?;
        Self::wrap(socket, false)
    }

    pub fn open_v6() -> io::Result<Self> {
        let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_only_v6(true)?;
        share_port(&socket)?;
        socket.set_multicast_hops_v6(255)?;
        socket.set_multicast_loop_v6(true)?;
        setsockopt(&socket, sockopt::Ipv6RecvPacketInfo, &true).map_err(io::Error::from)?;
        socket.bind(&SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, MDNS_PORT, 0, 0)).into())?;
        Self::wrap(socket, true)
    }

    fn wrap(socket: Socket, v6: bool) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        // SAFETY: the `Socket` owns its descriptor and is moved into the
        // `AsyncFd`, which keeps it open until the registration is dropped.
        let fd = unsafe { AsyncFd::register(socket)? };
        Ok(Self { fd, v6 })
    }

    pub fn is_v6(&self) -> bool {
        self.v6
    }

    pub fn join(&self, iface: &Interface) -> io::Result<()> {
        if self.v6 {
            self.fd.get_ref().join_multicast_v6(&GROUP_V6, iface.index)
        } else {
            self.fd
                .get_ref()
                .join_multicast_v4_n(&GROUP_V4, &v4_membership(iface)?)
        }
    }

    pub fn leave(&self, iface: &Interface) -> io::Result<()> {
        if self.v6 {
            self.fd.get_ref().leave_multicast_v6(&GROUP_V6, iface.index)
        } else {
            self.fd
                .get_ref()
                .leave_multicast_v4_n(&GROUP_V4, &v4_membership(iface)?)
        }
    }

    /// Sends a message to the mDNS group out of one interface.
    pub fn send_to_group(&self, bytes: &[u8], iface: &Interface) -> io::Result<()> {
        let socket = self.fd.get_ref();
        let dest = if self.v6 {
            socket.set_multicast_if_v6(iface.index)?;
            SocketAddr::V6(SocketAddrV6::new(GROUP_V6, MDNS_PORT, 0, iface.index))
        } else {
            let Some(addr) = iface.v4.first() else {
                return Ok(());
            };
            socket.set_multicast_if_v4(addr)?;
            SocketAddr::V4(SocketAddrV4::new(GROUP_V4, MDNS_PORT))
        };
        socket.send_to(bytes, &dest.into()).map(|_| ())
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<Received> {
        loop {
            let mut guard = self.fd.readable().await?;
            match guard.try_io(|inner| recv_with_ifindex(inner.get_ref().as_raw_fd(), buf)) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }
}

/// How to name the interface in an IPv4 membership request.
///
/// Linux accepts the interface index (`ip_mreqn`). macOS does not reliably
/// honour it, so the interface's first IPv4 address is used there instead.
fn v4_membership(iface: &Interface) -> io::Result<InterfaceIndexOrAddress> {
    if cfg!(target_os = "macos") {
        let addr = iface.v4.first().ok_or_else(|| {
            io::Error::new(io::ErrorKind::AddrNotAvailable, "interface has no IPv4 address")
        })?;
        Ok(InterfaceIndexOrAddress::Address(*addr))
    } else {
        Ok(InterfaceIndexOrAddress::Index(iface.index))
    }
}

fn share_port(socket: &Socket) -> io::Result<()> {
    socket.set_reuse_address(true)?;
    socket.set_reuse_port(true)
}

// `ipi_ifindex` is a `c_int` on Linux and a `c_uint` on macOS.
#[allow(clippy::unnecessary_cast)]
fn recv_with_ifindex(fd: std::os::fd::RawFd, buf: &mut [u8]) -> io::Result<Received> {
    let mut cmsg = nix::cmsg_space!(libc::in6_pktinfo);
    let mut iov = [IoSliceMut::new(buf)];
    let msg = recvmsg::<SockaddrStorage>(fd, &mut iov, Some(&mut cmsg), MsgFlags::empty())
        .map_err(io::Error::from)?;
    let mut ifindex = 0;
    for c in msg.cmsgs().map_err(io::Error::from)? {
        match c {
            ControlMessageOwned::Ipv4PacketInfo(info) => ifindex = info.ipi_ifindex as u32,
            ControlMessageOwned::Ipv6PacketInfo(info) => ifindex = info.ipi6_ifindex,
            _ => {}
        }
    }
    let src = msg
        .address
        .and_then(|a| {
            a.as_sockaddr_in()
                .map(|s| IpAddr::V4(s.ip()))
                .or_else(|| a.as_sockaddr_in6().map(|s| IpAddr::V6(s.ip())))
        })
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    Ok(Received {
        len: msg.bytes,
        src,
        ifindex,
    })
}
