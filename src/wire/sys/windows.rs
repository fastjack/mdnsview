//! Socket and interface code for Windows.

use std::io;
use std::mem::{offset_of, size_of};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::windows::io::AsRawSocket;

use socket2::{Domain, InterfaceIndexOrAddress, Protocol, SockRef, Socket, Type};
use tokio::io::Interest;
use tokio::net::UdpSocket;

use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    GetAdaptersAddresses, IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
    IP_ADAPTER_NO_MULTICAST,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, CMSGHDR, IN_PKTINFO, IN6_PKTINFO, IP_PKTINFO, IPPROTO_IP,
    IPPROTO_IPV6, IPV6_PKTINFO, LPFN_WSARECVMSG, LPWSAOVERLAPPED_COMPLETION_ROUTINE,
    SIO_GET_EXTENSION_FUNCTION_POINTER, SIO_UDP_CONNRESET, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6,
    SOCKADDR_STORAGE, SOCKET, WSABUF, WSAID_WSARECVMSG, WSAIoctl, WSAMSG, setsockopt,
};
use windows_sys::Win32::System::IO::OVERLAPPED;
use windows_sys::core::GUID;

use super::{GROUP_V4, GROUP_V6, MDNS_PORT, Received};
use crate::wire::interfaces::Interface;

/// One adapter as reported by `GetAdaptersAddresses`, copied out of the
/// system buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Adapter {
    friendly_name: String,
    if_index: u32,
    ipv6_if_index: u32,
    oper_up: bool,
    flags: u32,
    if_type: u32,
    addresses: Vec<IpAddr>,
}

/// Maps an adapter to an `Interface`; `None` when it has no usable index.
fn to_interface(a: Adapter) -> Option<Interface> {
    // `IfIndex` is 0 when IPv4 is disabled on the adapter; on Windows both
    // indexes are the same number when both exist.
    let index = if a.if_index != 0 { a.if_index } else { a.ipv6_if_index };
    if index == 0 {
        return None;
    }
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for addr in a.addresses {
        match addr {
            IpAddr::V4(a) => v4.push(a),
            IpAddr::V6(a) => v6.push(a),
        }
    }
    Some(Interface {
        name: a.friendly_name,
        index,
        up: a.oper_up,
        multicast: a.flags & IP_ADAPTER_NO_MULTICAST == 0,
        loopback: a.if_type == IF_TYPE_SOFTWARE_LOOPBACK,
        v4,
        v6,
    })
}

/// Lists all interfaces with their flags and addresses, named by their
/// connection name (`WLAN`, `Ethernet 2`).
pub fn list_interfaces() -> io::Result<Vec<Interface>> {
    let mut interfaces: Vec<Interface> = adapters()?.into_iter().filter_map(to_interface).collect();
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(interfaces)
}

fn adapters() -> io::Result<Vec<Adapter>> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    // Microsoft recommends starting with 15 KB and growing on overflow.
    let mut size: u32 = 15 * 1024;
    // u64 elements keep the buffer aligned for the structs written into it.
    let mut buf: Vec<u64>;
    loop {
        buf = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: `buf` has room for `size` bytes, and the call writes at most that.
        let rc = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC as u32,
                flags,
                std::ptr::null(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        match rc {
            NO_ERROR => break,
            ERROR_BUFFER_OVERFLOW => continue,
            err => return Err(io::Error::from_raw_os_error(err as i32)),
        }
    }
    let mut out = Vec::new();
    let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !p.is_null() {
        // SAFETY: `p` points into `buf`, into the linked list the call filled in.
        let a = unsafe { &*p };
        let mut addresses = Vec::new();
        let mut u = a.FirstUnicastAddress;
        while !u.is_null() {
            // SAFETY: as above, the unicast list lives in `buf`.
            let ua = unsafe { &*u };
            if let Some(ip) = unsafe { sockaddr_ip(ua.Address.lpSockaddr) } {
                addresses.push(ip);
            }
            u = ua.Next;
        }
        out.push(Adapter {
            // SAFETY: `FriendlyName` is a NUL-terminated wide string in `buf`.
            friendly_name: unsafe { wide(a.FriendlyName) },
            // SAFETY: the union is the `Length`/`IfIndex` pair on every Windows
            // version; `Alignment` only forces 8-byte alignment.
            if_index: unsafe { a.Anonymous1.Anonymous.IfIndex },
            ipv6_if_index: a.Ipv6IfIndex,
            oper_up: a.OperStatus == IfOperStatusUp,
            // SAFETY: `Flags` covers the whole bitfield union.
            flags: unsafe { a.Anonymous2.Flags },
            if_type: a.IfType,
            addresses,
        });
        p = a.Next;
    }
    Ok(out)
}

/// Reads a NUL-terminated UTF-16 string.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated wide string.
pub(crate) unsafe fn wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: the caller guarantees a terminating NUL.
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: the `len` units before the NUL are readable.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

/// The IP address in a socket address, if it is IPv4 or IPv6.
///
/// # Safety
/// `sa` must be null or point to a socket address of the length its family implies.
unsafe fn sockaddr_ip(sa: *const SOCKADDR) -> Option<IpAddr> {
    if sa.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees the address matches its family.
    unsafe {
        match (*sa).sa_family {
            AF_INET => {
                let sin = &*(sa as *const SOCKADDR_IN);
                Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(sin.sin_addr.S_un.S_addr))))
            }
            AF_INET6 => {
                let sin6 = &*(sa as *const SOCKADDR_IN6);
                Some(IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.u.Byte)))
            }
            _ => None,
        }
    }
}

/// One mDNS socket (IPv4 or IPv6) bound to port 5353, shared with other
/// mDNS software on the host.
pub struct MdnsSocket {
    udp: UdpSocket,
    v6: bool,
    recv_msg: WsaRecvMsg,
}

type WsaRecvMsg = unsafe extern "system" fn(
    SOCKET,
    *mut WSAMSG,
    *mut u32,
    *mut OVERLAPPED,
    LPWSAOVERLAPPED_COMPLETION_ROUTINE,
) -> i32;

impl MdnsSocket {
    pub fn open_v4() -> io::Result<Self> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        // On Windows, SO_REUSEADDR alone lets every socket on the port
        // receive the group's datagrams; there is no SO_REUSEPORT.
        socket.set_reuse_address(true)?;
        socket.set_multicast_ttl_v4(255)?;
        socket.set_multicast_loop_v4(true)?;
        enable(&socket, IPPROTO_IP, IP_PKTINFO)?;
        socket.bind(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MDNS_PORT)).into())?;
        Self::wrap(socket, false)
    }

    pub fn open_v6() -> io::Result<Self> {
        let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_only_v6(true)?;
        socket.set_reuse_address(true)?;
        socket.set_multicast_hops_v6(255)?;
        socket.set_multicast_loop_v6(true)?;
        enable(&socket, IPPROTO_IPV6, IPV6_PKTINFO)?;
        socket.bind(&SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, MDNS_PORT, 0, 0)).into())?;
        Self::wrap(socket, true)
    }

    fn wrap(socket: Socket, v6: bool) -> io::Result<Self> {
        disable_connreset(&socket)?;
        let recv_msg = wsa_recvmsg(&socket)?;
        socket.set_nonblocking(true)?;
        let udp = UdpSocket::from_std(socket.into())?;
        Ok(Self { udp, v6, recv_msg })
    }

    pub fn is_v6(&self) -> bool {
        self.v6
    }

    pub fn join(&self, iface: &Interface) -> io::Result<()> {
        let socket = SockRef::from(&self.udp);
        if self.v6 {
            socket.join_multicast_v6(&GROUP_V6, iface.index)
        } else {
            socket.join_multicast_v4_n(&GROUP_V4, &InterfaceIndexOrAddress::Index(iface.index))
        }
    }

    pub fn leave(&self, iface: &Interface) -> io::Result<()> {
        let socket = SockRef::from(&self.udp);
        if self.v6 {
            socket.leave_multicast_v6(&GROUP_V6, iface.index)
        } else {
            socket.leave_multicast_v4_n(&GROUP_V4, &InterfaceIndexOrAddress::Index(iface.index))
        }
    }

    /// Sends a message to the mDNS group out of one interface.
    pub fn send_to_group(&self, bytes: &[u8], iface: &Interface) -> io::Result<()> {
        let socket = SockRef::from(&self.udp);
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
        let raw = self.udp.as_raw_socket() as SOCKET;
        self.udp
            .async_io(Interest::READABLE, || recv_with_ifindex(raw, self.recv_msg, buf))
            .await
    }
}

/// Turns on a boolean socket option.
fn enable(socket: &Socket, level: i32, name: i32) -> io::Result<()> {
    let on: u32 = 1;
    // SAFETY: a valid socket and a 4-byte option value.
    let rc = unsafe {
        setsockopt(
            socket.as_raw_socket() as SOCKET,
            level,
            name,
            (&on as *const u32).cast(),
            size_of::<u32>() as i32,
        )
    };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Without this, an ICMP port-unreachable for an earlier send makes the
/// next receive fail with WSAECONNRESET.
fn disable_connreset(socket: &Socket) -> io::Result<()> {
    let off: u32 = 0;
    let mut returned = 0u32;
    // SAFETY: a valid socket and a 4-byte input buffer; no output.
    let rc = unsafe {
        WSAIoctl(
            socket.as_raw_socket() as SOCKET,
            SIO_UDP_CONNRESET,
            (&off as *const u32).cast(),
            size_of::<u32>() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
            None,
        )
    };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// `WSARecvMsg` is not exported; Winsock hands out its address per socket.
fn wsa_recvmsg(socket: &Socket) -> io::Result<WsaRecvMsg> {
    let mut f: LPFN_WSARECVMSG = None;
    let mut returned = 0u32;
    // SAFETY: a valid socket, the GUID as input and a function pointer as output.
    let rc = unsafe {
        WSAIoctl(
            socket.as_raw_socket() as SOCKET,
            SIO_GET_EXTENSION_FUNCTION_POINTER,
            (&WSAID_WSARECVMSG as *const GUID).cast(),
            size_of::<GUID>() as u32,
            (&mut f as *mut LPFN_WSARECVMSG).cast(),
            size_of::<LPFN_WSARECVMSG>() as u32,
            &mut returned,
            std::ptr::null_mut(),
            None,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    f.ok_or_else(|| io::Error::other("WSARecvMsg not available"))
}

fn recv_with_ifindex(socket: SOCKET, recv_msg: WsaRecvMsg, buf: &mut [u8]) -> io::Result<Received> {
    // SAFETY: an all-zero SOCKADDR_STORAGE is a valid empty address.
    let mut from: SOCKADDR_STORAGE = unsafe { std::mem::zeroed() };
    // u64 elements align the control buffer for CMSGHDR.
    let mut control = [0u64; 16];
    let mut data = WSABUF {
        len: buf.len().min(u32::MAX as usize) as u32,
        buf: buf.as_mut_ptr(),
    };
    let mut msg = WSAMSG {
        name: (&mut from as *mut SOCKADDR_STORAGE).cast(),
        namelen: size_of::<SOCKADDR_STORAGE>() as i32,
        lpBuffers: &mut data,
        dwBufferCount: 1,
        Control: WSABUF {
            len: size_of_val(&control) as u32,
            buf: control.as_mut_ptr().cast(),
        },
        dwFlags: 0,
    };
    let mut len = 0u32;
    // SAFETY: every pointer in `msg` refers to a live local buffer of the stated size.
    let rc = unsafe { recv_msg(socket, &mut msg, &mut len, std::ptr::null_mut(), None) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the used part of `control` is `msg.Control.len` bytes long.
    let used = unsafe {
        std::slice::from_raw_parts(control.as_ptr().cast::<u8>(), msg.Control.len as usize)
    };
    // SAFETY: Winsock wrote a socket address of its family into `from`.
    let src = unsafe { sockaddr_ip((&from as *const SOCKADDR_STORAGE).cast()) }
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    Ok(Received {
        len: len as usize,
        src,
        ifindex: pktinfo_ifindex(used).unwrap_or(0),
    })
}

/// Rounds up to the alignment of control message headers and data
/// (`WSA_CMSGHDR_ALIGN` / `WSA_CMSGDATA_ALIGN`: the alignment of a pointer).
fn cmsg_align(n: usize) -> usize {
    n.next_multiple_of(align_of::<usize>())
}

/// The receiving interface from an `IP_PKTINFO` or `IPV6_PKTINFO` control
/// message in a `WSARecvMsg` control buffer.
fn pktinfo_ifindex(control: &[u8]) -> Option<u32> {
    let bytes = |at: usize, n: usize| control.get(at..at + n);
    let read_usize = |at| bytes(at, size_of::<usize>()).map(|b| usize::from_ne_bytes(b.try_into().unwrap()));
    let read_u32 = |at| bytes(at, 4).map(|b| u32::from_ne_bytes(b.try_into().unwrap()));
    let mut offset = 0;
    while offset + size_of::<CMSGHDR>() <= control.len() {
        let len = read_usize(offset + offset_of!(CMSGHDR, cmsg_len))?;
        let level = read_u32(offset + offset_of!(CMSGHDR, cmsg_level))? as i32;
        let kind = read_u32(offset + offset_of!(CMSGHDR, cmsg_type))? as i32;
        if len < size_of::<CMSGHDR>() {
            return None;
        }
        let data = offset + cmsg_align(size_of::<CMSGHDR>());
        match (level, kind) {
            (IPPROTO_IP, IP_PKTINFO) => return read_u32(data + offset_of!(IN_PKTINFO, ipi_ifindex)),
            (IPPROTO_IPV6, IPV6_PKTINFO) => {
                return read_u32(data + offset_of!(IN6_PKTINFO, ipi6_ifindex));
            }
            _ => offset += cmsg_align(len),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(name: &str, index: u32) -> Adapter {
        Adapter {
            friendly_name: name.into(),
            if_index: index,
            ipv6_if_index: index,
            oper_up: true,
            flags: 0,
            if_type: 71, // IF_TYPE_IEEE80211
            addresses: vec![
                IpAddr::V4(Ipv4Addr::new(192, 168, 4, 20)),
                IpAddr::V6("fe80::1".parse().unwrap()),
            ],
        }
    }

    #[test]
    fn connected_wifi() {
        let i = to_interface(adapter("WLAN", 14)).unwrap();
        assert_eq!(i.name, "WLAN");
        assert_eq!(i.index, 14);
        assert!(i.up && i.multicast && !i.loopback);
        assert_eq!(i.v4, vec![Ipv4Addr::new(192, 168, 4, 20)]);
        assert_eq!(i.v6, vec!["fe80::1".parse::<Ipv6Addr>().unwrap()]);
    }

    #[test]
    fn disconnected_adapter_is_down() {
        let a = Adapter {
            oper_up: false,
            addresses: Vec::new(),
            ..adapter("Bluetooth Network Connection", 12)
        };
        assert!(!to_interface(a).unwrap().up);
    }

    #[test]
    fn loopback_pseudo_interface() {
        let a = Adapter {
            if_type: IF_TYPE_SOFTWARE_LOOPBACK,
            ..adapter("Loopback Pseudo-Interface 1", 1)
        };
        assert!(to_interface(a).unwrap().loopback);
    }

    #[test]
    fn no_multicast_flag() {
        let a = Adapter {
            flags: IP_ADAPTER_NO_MULTICAST,
            ..adapter("Teredo", 9)
        };
        assert!(!to_interface(a).unwrap().multicast);
    }

    #[test]
    fn ipv6_only_adapter_uses_ipv6_index() {
        let a = Adapter {
            if_index: 0,
            ipv6_if_index: 31,
            ..adapter("Ethernet 2", 0)
        };
        assert_eq!(to_interface(a).unwrap().index, 31);
        let none = Adapter {
            if_index: 0,
            ipv6_if_index: 0,
            ..adapter("Ghost", 0)
        };
        assert!(to_interface(none).is_none());
    }

    /// Builds a control buffer with one message of the given level, type and data.
    fn control(level: i32, kind: i32, data: &[u8]) -> Vec<u8> {
        let header = cmsg_align(size_of::<CMSGHDR>());
        let mut buf = vec![0u8; header + cmsg_align(data.len())];
        let len = size_of::<CMSGHDR>() + data.len();
        buf[offset_of!(CMSGHDR, cmsg_len)..][..size_of::<usize>()].copy_from_slice(&len.to_ne_bytes());
        buf[offset_of!(CMSGHDR, cmsg_level)..][..4].copy_from_slice(&level.to_ne_bytes());
        buf[offset_of!(CMSGHDR, cmsg_type)..][..4].copy_from_slice(&kind.to_ne_bytes());
        buf[header..header + data.len()].copy_from_slice(data);
        buf
    }

    #[test]
    fn ipv4_pktinfo() {
        let mut info = vec![0u8; size_of::<IN_PKTINFO>()];
        info[..4].copy_from_slice(&[224, 0, 0, 251]);
        info[offset_of!(IN_PKTINFO, ipi_ifindex)..][..4].copy_from_slice(&14u32.to_ne_bytes());
        assert_eq!(pktinfo_ifindex(&control(IPPROTO_IP, IP_PKTINFO, &info)), Some(14));
    }

    #[test]
    fn ipv6_pktinfo_after_other_message() {
        let mut info = vec![0u8; size_of::<IN6_PKTINFO>()];
        info[offset_of!(IN6_PKTINFO, ipi6_ifindex)..][..4].copy_from_slice(&28u32.to_ne_bytes());
        let mut buf = control(IPPROTO_IPV6, 21, &[1, 2, 3, 4]); // IPV6_HOPLIMIT
        buf.extend(control(IPPROTO_IPV6, IPV6_PKTINFO, &info));
        assert_eq!(pktinfo_ifindex(&buf), Some(28));
    }

    #[test]
    fn no_pktinfo() {
        assert_eq!(pktinfo_ifindex(&[]), None);
        assert_eq!(pktinfo_ifindex(&control(IPPROTO_IP, 99, &[0; 4])), None);
        let mut broken = control(IPPROTO_IP, IP_PKTINFO, &[0; 8]);
        broken[..size_of::<usize>()].fill(0); // a zero cmsg_len must not loop forever
        assert_eq!(pktinfo_ifindex(&broken), None);
    }

    /// Prints the interface list: `cargo test live_windows_interfaces -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the real interface list"]
    fn live_windows_interfaces() {
        for i in list_interfaces().unwrap() {
            println!(
                "{:>3} {:<40} up={} mc={} lo={} v4={:?} v6={:?}",
                i.index, i.name, i.up, i.multicast, i.loopback, i.v4, i.v6
            );
        }
    }
}
