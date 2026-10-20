# Design

## Context

See proposal.md for the motivation. The required behavior is in the deltas for `mdns-discovery`, `app-configuration` and `browser-ui`.

**The build today.** `cargo check` on Windows 11 23H2 (build 22631, x86_64-pc-windows-msvc, Rust 1.99.1) fails in three places:
- `src/wire/sys.rs`: `nix`, `std::os::fd`, `tokio::io::unix::AsyncFd`, `Socket::set_reuse_port`
- `src/ui/view.rs`: `clock`, which uses `libc::localtime_r`
- `src/backend/dnssd.rs`: compiled only on macOS, so not one of the failures

Everything else compiles: the store, UI, codec, scheduler and config. `src/wire/backend.rs` only uses `libc::EHOSTUNREACH`, and that constant exists in `libc` on Windows. `src/ui/app.rs` already ignores `KeyEventKind::Release`, so crossterm's Windows key-release events do not act twice.

**The development PC.** These facts shape the design:
- UDP 5353 is bound by `svchost` (the DNS Client service, `Dnscache`) on `::`, and by `msedge` on `0.0.0.0`. So sharing the port works at least for IPv4.
- Bonjour for Windows is not installed.
- The connections are `WLAN` (up, index 14), `Tailscale` (up, index 28) and `Bluetooth-Netzwerkverbindung 2` (disconnected). The Windows UI is in German, so connection names are localized.
- The built-in "mDNS (UDP-In)" firewall rules allow only `svchost.exe` with service `dnscache`, on all profiles. `WLAN` is on the Public profile.

**Where the platform split is today.** `src/backend/mod.rs` selects the daemon backend by `cfg(target_os)`, and Windows currently gets `NullBackend`. `WireBackend` uses only the `sys` API: `list_interfaces`, `MdnsSocket::{open_v4, open_v6, join, leave, send_to_group, recv}`, and `Received { len, src, ifindex }`.

## Goals / Non-Goals

**Goals:**
- One code base that builds with plain `cargo build` on Linux, macOS and Windows (MSVC toolchain), with no feature flags.
- On Windows, behavior that matches the other platforms: two sources, the same observations and the same UI. The one addition is the firewall hint.
- The Windows binary starts on every supported Windows 10/11 build, even one without the DNS-SD API.

**Non-Goals:**
- The GNU (`windows-gnu`) toolchain. It may work, but it is not checked.
- Rewriting the Unix socket code to share an implementation with Windows. The two `sys` modules share an API, not code.
- Exact TXT fidelity from the Windows API (see Decision 3). The wire source provides exact TXT data.

## Decisions

### 1. `wire::sys` becomes two modules with one API

- `src/wire/sys.rs` is renamed to `src/wire/sys/unix.rs` (`#[cfg(unix)]`), unchanged.
- A new `src/wire/sys/windows.rs` (`#[cfg(windows)]`) provides the same public items.
- `src/wire/sys/mod.rs` re-exports whichever module applies, and holds the shared constants `MDNS_PORT`, `GROUP_V4`, `GROUP_V6` and the `Received` struct.
- `live_wire_ifindex` moves to `mod.rs` so that it runs on all three platforms. Its default interface is `en0` on macOS, `WLAN` on Windows (override with `MDNSVIEW_LIVE_IFACE`), and `eth0` elsewhere.

*Alternative considered:* `cfg` branches inside one file, as was done for macOS. Rejected: on Windows almost every function differs (interfaces, receive path, async registration), so one file would mix two implementations line by line.

### 2. Windows socket: `socket2` + tokio `UdpSocket` + `WSARecvMsg`

**Opening.**
- `socket2` creates the socket. On Windows, `SO_REUSEADDR` alone lets several sockets bind 5353 and all receive the multicast datagrams. That is how Chrome and Edge share the port with the DNS Client, so `share_port` there sets only `set_reuse_address(true)`.
- Packet info is turned on with `setsockopt(IP_PKTINFO)` / `setsockopt(IPV6_PKTINFO)` through `windows-sys`, since `socket2` has no setter for them.
- `SIO_UDP_CONNRESET` is set to off. Otherwise an ICMP port-unreachable makes the next receive fail with `WSAECONNRESET`.
- The socket is then converted into `std::net::UdpSocket` and `tokio::net::UdpSocket::from_std`, which registers it with tokio's Windows reactor.

**Receiving.** `recv` uses `UdpSocket::async_io(Interest::READABLE, …)`, calling `WSARecvMsg` on the raw `SOCKET` with a control buffer that is big enough for `IN6_PKTINFO`.
- `WSARecvMsg` has no import. Its pointer is fetched once per socket with `WSAIoctl(SIO_GET_EXTENSION_FUNCTION_POINTER, WSAID_WSARECVMSG)`.
- The receiving interface comes from `IN_PKTINFO.ipi_ifindex` or `IN6_PKTINFO.ipi6_ifindex`.
- The source address comes from the returned `SOCKADDR_STORAGE`.
- `WSAEMSGSIZE` (datagram bigger than the 9000-byte buffer) is returned as an error. `WireBackend::run` already logs receive errors and continues.

**Joining and sending.**
- IPv4 joins use the interface index (`InterfaceIndexOrAddress::Index`). Windows has supported `IP_ADD_MEMBERSHIP` with an index in `imr_interface` (the `0.0.0.x` form) since Vista. `socket2` already uses `ip_mreq` by index on Windows. The interface-address fallback used on macOS is kept ready behind `cfg(windows)` in case the live test shows otherwise.
- IPv6 joins use `join_multicast_v6(&GROUP_V6, index)`.
- Sends go through `socket2::SockRef::from(&udp)`, with the same `set_multicast_if_*` + `send_to` sequence as on Unix.

*Alternatives considered:*
- One socket per interface, each bound to that interface's address. This would avoid `WSARecvMsg`. Rejected: binding to a unicast address does not receive group traffic reliably on Windows, and the number of sockets would grow with the interface list.
- Raw sockets or Npcap. These need administrator rights or a driver, which the spec rules out.

### 3. Windows interfaces from `GetAdaptersAddresses`

`list_interfaces` calls `GetAdaptersAddresses(AF_UNSPEC, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER)`. The buffer starts at 15 KB and grows on `ERROR_BUFFER_OVERFLOW`, as Microsoft recommends. Each adapter becomes an `Interface`:

| `Interface` field | Source |
|---|---|
| `name` | `FriendlyName`, the connection name the user sees and can rename (`WLAN`, `Ethernet 2`, `vEthernet (WSL)`) |
| `index` | `IfIndex`, or `Ipv6IfIndex` when IPv4 is disabled on the adapter (`IfIndex == 0`). On Windows both are the same number when both exist. |
| `up` | `OperStatus == IfOperStatusUp` |
| `multicast` | the `IP_ADAPTER_NO_MULTICAST` flag is not set |
| `loopback` | `IfType == IF_TYPE_SOFTWARE_LOOPBACK` |
| `v4` / `v6` | the unicast address list |

Using the adapter's GUID (`AdapterName`) as the name was rejected, because nobody can write a pattern for it. The default exclude list stays the same on Windows. None of its patterns match Windows names. Disconnected adapters drop out because they are not up. A tunnel such as Tailscale is up and gets joined, as on the other platforms. If the join fails, that is logged and ignored. Users who want it skipped add a pattern, as the README will show. The 5-second interface poll in `WireBackend` works the same way, so change detection needs no Windows code.

### 4. Windows DNS backend (`src/backend/windns.rs`)

**Loading the API.** The module is compiled only on `cfg(windows)`. `windows-sys` 0.61 links its functions through `windows-link` (raw-dylib), and a missing export would then stop the program at load time. The backend therefore resolves `DnsServiceBrowse`, `DnsServiceBrowseCancel`, `DnsServiceResolve`, `DnsServiceResolveCancel`, `DnsServiceFreeInstance` and `DnsFree` itself, with `GetModuleHandle`/`LoadLibraryW("dnsapi.dll")` + `GetProcAddress`. It uses the struct and callback types from `windows-sys`. If any function is missing, it reports `Unavailable("Windows DNS-SD API not available (needs Windows 10 1809 or later)")` and returns.

**The flow** mirrors the mDNSResponder backend. It was adjusted after the spike in task 4.1 (see notes.md):

1. **Service types.** `DnsServiceBrowse("_services._dns-sd._udp.local")`, request version 1 with `pBrowseCallback`. The callback receives a `DNS_RECORD` list. Each PTR record names a service type and produces the meta-PTR observation. A browse is started for each new type.
2. **Instances.** A type browse's callback holds PTR records and often also the instance's SRV, TXT, A and AAAA records:
   - Every PTR produces the PTR observation and starts one `DnsServiceResolve` for the instance.
   - The other records become observations directly. TXT strings are taken as they are, so they are exact.
   - A goodbye for the PTR means the instance was removed: the backend stops its resolver and sends `SystemRemoved`. Other goodbye records are not observed.
   - The record's `dwTtl` is not a TTL but an expiry time on the DNS Client's internal clock (clock + TTL; measured in task 4.4). A goodbye therefore shows as the expiry dropping to "now". The tracker keeps each record's last expiry, and treats a value of 0 or a drop of more than 1 s as a goodbye.
   - The API reports no interface for a browse result, so instances are tracked by name only, not per interface as on macOS.
3. **Resolve.** It adds what browse results lack. The callback gets a `DNS_SERVICE_INSTANCE`, which produces:
   - SRV (host, port, priority, weight)
   - A from `ip4Address` and AAAA from `ip6Address`, when present
   - TXT rebuilt as `key=value` strings (or `key` for an empty or null value), only when the instance's browse delivered no TXT, because the rebuilt form is not exact

   The interface name comes from `dwInterfaceIndex`, through the same `list_interfaces` lookup the wire source uses. The tracker remembers it per instance and uses it for that instance's later browse records. The instance is freed with `DnsServiceFreeInstance`. Resolves that never answer are harmless, because browse already supplied the records.
4. **Observations.** All are `Source::System` with `ttl: None`, as for the other daemons.
5. **Status.**
   - `Active` is sent once the first browse returns `DNS_REQUEST_PENDING`.
   - If that call fails, the backend reports `Unavailable` with the Win32 error text.
   - Later per-operation errors are logged at debug level, and that operation is dropped.

**Threading.** The callbacks run on Windows thread-pool threads. As in `dnssd.rs`, they only queue events:
- Each operation's context is a boxed `OpContext` holding an `UnboundedSender<Event>` and the operation's key. The box is freed in the operation's final callback: `ERROR_CANCELLED` for a browse, the one completion for a resolve. A late callback therefore never sees freed memory.
- One async task owns the `Tracker` and the cancel handles. It turns events into observations and commands, starts and cancels operations, and forwards observations to the store channel.
- When the store channel closes, it cancels every operation and returns.

**Browse lifetime.** A browse keeps reporting until it is cancelled (measured in task 4.1), so no restarts are needed.

**Shared name helpers.** `dnssd.rs` has `parse_presentation`, `instance_name` and `split_txt`. The first two move to `src/backend/names.rs` (`cfg(any(target_os = "macos", windows))`) so that both backends use them. Their tests move along with them.

The backend name is `"Windows DNS"`.

*Alternatives considered:*
- `DnsQueryEx` with `DNS_QUERY_MULTICAST_ONLY`: a one-shot query with no ongoing browse, so removals and new services would need polling.
- Bonjour's `dnssd.dll`: the user decided against it. It is not installed on most PCs.

### 5. Firewall silence detection

**What the firewall does, as measured (see notes.md).** Without an allow rule, and even with the Block rules Windows creates when its prompt is dismissed, Windows Firewall lets traffic in for some seconds after the socket's own multicast sends. Unsolicited announcements are dropped. mdnsview queries continuously, so its wire source keeps receiving on this PC. The detection below therefore cannot be triggered here. The user chose to keep it as a safeguard for setups that also drop replies, such as stricter policies or third-party firewalls. The README explains the allow rule, which also lets announcements and goodbyes in.

The decision needs two facts that live in different places:
- whether wire traffic arrived from another host (the wire backend knows)
- whether the daemon found services (the store knows)

So the logic is split:

1. **Wire backend (`cfg(windows)`).**
   - On each `sync_interfaces`, it remembers the addresses of all listed interfaces as "own addresses", including interfaces that are not selected.
   - Every response from an address outside that set updates `last_foreign`.
   - On each 5-second interface poll tick: if 15 s have passed since the first successful send and `last_foreign` is unset or older than 15 s, it sends `Observation::WireSilent(true)` once. The first packet from another host after that sends `WireSilent(false)`.
   - The wire source state stays `Active` throughout, because the socket works and queries go out.

   After the first reply, silence is measured from the last foreign packet. A busy LAN never stays quiet for 15 s once traffic flows, and a quiet one only shows the hint while Windows DNS keeps listing services.

2. **Store.** It records the flag in `Status::wire_silent`.

3. **`Status::sources_label`.** It appends ` (no replies: check Windows Firewall)` when `wire_silent` is set, the system source is `Active`, and the snapshot has at least one service instance from `Source::System`.

   The detection function (silence plus timing) and the label rule are pure functions, so they are unit-tested. The interface poll drives the detection, so no extra timer is needed. The scheduler tick was the first plan, but once enumeration settles it can go minutes without a due query.

*Alternatives considered:*
- Mark the wire source `Unavailable`, as macOS does for Local Network. Rejected: the socket works, and the next allow rule takes effect without a restart. "Unavailable" would also turn the label into "system only", which hides that the queries still go out.
- Query the firewall configuration through `INetFwPolicy2` (COM). This would add a COM dependency and still miss third-party firewalls. The silence heuristic catches both.

### 6. Small portability fixes

- **`clock` in `src/ui/view.rs`:** uses `libc::localtime_s(&mut tm, &secs)` on Windows (note the swapped argument order and the `errno_t` result) and `localtime_r` elsewhere, in a two-arm `cfg` helper.
- **`local_network_blocked`:** the call site in `src/wire/backend.rs` keeps its runtime `cfg!(target_os = "macos")` check. It already compiles on Windows, since `libc::EHOSTUNREACH` exists on all targets. A `#[cfg]` would leave the `blocked` flag, the send results and the function unused on Linux and need extra `cfg` attributes to silence them.
- **Dependencies:**
  - `nix` moves to `[target.'cfg(unix)'.dependencies]`.
  - `[target.'cfg(windows)'.dependencies]` adds `windows-sys = "0.61"` with the features `Win32_Foundation`, `Win32_Networking_WinSock`, `Win32_NetworkManagement_IpHelper`, `Win32_NetworkManagement_Ndis`, `Win32_NetworkManagement_Dns`, `Win32_System_IO` and `Win32_System_LibraryLoader`.
  - `NullBackend`'s dead-code `cfg_attr` widens to include Windows.
- **Paths:** no code change. `directories` 6 on Windows returns `{RoamingAppData}\mdnsview\config` as `config_dir`, no `state_dir`, and `{LocalAppData}\mdnsview\cache` as `cache_dir`. A run on the PC confirms this before the spec and README state it (task 6.2).

## Risks / Trade-offs

- **[The DNS Client binds `[::]:5353` exclusively, so our IPv6 socket cannot bind.]** This is unknown until tested. `WireBackend::open` already continues with IPv4 only and logs a warning. The README states the limitation if it holds.
- **[`WSARecvMsg` packet info is wrong for multicast on some adapters (ifindex 0).]** `handle_packet` would then silently drop the packets. The live test asserts the right index first. The fallback is to bind one socket per joined interface's address only for the receive path; it is not built unless that test fails.
- **[The Windows DNS-SD API's browse semantics, TTL-0 removal and service-type enumeration are badly documented.]** Resolved by the spike in task 4.1: browses run until cancelled, goodbyes arrive as TTL-0 records, and type enumeration works.
- **[TXT data from `DNS_SERVICE_INSTANCE` loses binary values and the difference between `key` and `key=`.]** This is mostly avoided, because browse results carry the exact TXT strings. The rebuilt form is used only when browse delivered none.
- **[The firewall heuristic misfires.]** On a LAN where the daemon lists only this PC's own services, wire packets come only from own addresses, which looks like silence. The label rule only checks that the daemon found some service instance, and the PC's own services count too. This is accepted as a rare false hint; the text says "check", not "blocked".
- **[Connection names are localized and renameable (`Bluetooth-Netzwerkverbindung 2`).]** This is accepted: the patterns are the user's. The README says to use the names shown by `Get-NetAdapter` or in Network Connections.
- **[Mouse and colors in the classic console (conhost).]** crossterm supports both. A manual check in Windows Terminal and conhost is part of task 7.1.

## Migration Plan

None needed. Linux and macOS configs and behavior are unchanged. To roll back, revert the change.

## Open Questions

- Whether the DNS Client service lets our socket share `[::]:5353` (see Risks). The answer changes only README wording and the IPv6 scenario's verification, not the design.
- The exact minimum Windows 10 build that exports `DnsServiceBrowse`. The runtime lookup makes this matter only for the wording of the unavailable reason.
