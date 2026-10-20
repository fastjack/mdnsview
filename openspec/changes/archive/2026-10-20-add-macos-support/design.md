# Design

## Context

See proposal.md for the motivation. The required behavior is in the deltas for `mdns-discovery` and `app-configuration`.

The code already separates the platform-specific parts:

- `src/wire/sys.rs` holds all socket and interface code. It uses `socket2`, `nix` and `libc`.
- `src/backend/mod.rs` picks the OS daemon backend with `cfg(target_os)`. Every platform except Linux falls back to `NullBackend`.
- `src/config.rs` and `src/main.rs` get their paths from `directories::ProjectDirs::from("", "", "mdnsview")`.

`cargo build` on macOS (Darwin, Apple Silicon) currently fails with one error: `Socket::set_multicast_all_v4` exists only on Linux. Everything else compiles, including `nix`'s `Ipv4PacketInfo` and `Ipv6RecvPacketInfo`. Whether they also work at runtime on macOS has not been checked yet.

A typical Mac lists many interfaces that are up and multicast-capable but useless for LAN discovery: `awdl0` and `llw0` (Apple Wireless Direct Link), many `utun*` (VPN and system tunnels), and `anpi*`. Thunderbolt bridge members `en1`–`en3` have no addresses.

## Goals / Non-Goals

**Goals:**
- One code base that builds with plain `cargo build` on Linux and macOS, with no feature flags.
- On macOS, behavior that matches Linux: two sources, the same observations, the same UI.

**Non-Goals:**
- Supporting macOS versions older than the ones the Rust toolchain targets by default.
- Moving code that both platforms share into new abstractions. `sys.rs` stays one Unix module with small `cfg` branches.

## Decisions

### 1. Socket options: `cfg` the Linux-only call, keep the rest

`set_multicast_all_v4(false)` exists only on Linux, so it moves behind `#[cfg(target_os = "linux")]`. BSD-derived stacks, macOS included, already deliver to a socket only the multicast groups that socket has joined, so macOS needs no replacement.

`IP_PKTINFO` and `IPV6_RECVPKTINFO` are supported by Darwin, and `nix` exposes both for Apple targets, so `recv_with_ifindex` stays as it is. Implementation checks that `ipi_ifindex` is filled in on macOS. The fallback is `IP_RECVIF` (`sockaddr_dl` cmsg), only if that check fails.

The IPv4 join uses `ip_mreqn` by interface index. Darwin's `IP_ADD_MEMBERSHIP` accepts `ip_mreqn`, but this is not well documented, so it is checked during implementation. Fallback: on macOS, join with `InterfaceIndexOrAddress::Address(first v4 address)`. The interface already has an IPv4 address whenever we join on v4, because of the check in `sync_interfaces`.

`SO_REUSEADDR` plus `SO_REUSEPORT` is how mDNSResponder itself shares 5353 with other clients, so `share_port` stays the same.

*Alternative considered:* a separate `sys_macos.rs`. Rejected because the differences are a few lines.

### 2. mDNSResponder backend through hand-written `dns_sd` FFI

The new module `src/backend/dnssd.rs` is compiled only on `target_os = "macos"`. It declares the dozen `dns_sd.h` functions it needs in an `extern "C"` block. They are in libSystem, so nothing extra needs to be linked.

The flow is the same as in the avahi backend:

1. `DNSServiceCreateConnection` sets up one shared connection. All later operations use `kDNSServiceFlagsShareConnection` on it, so one socket (`DNSServiceRefSockFD`) carries every reply.
2. `DNSServiceBrowse("_services._dns-sd._udp", "local.")` finds the service types. Each result arrives as instance `_ipp` with regtype `_tcp.local.`. It produces the `_services._dns-sd._udp.local` PTR observation, and starts one `DNSServiceBrowse` per new type.
3. For each type browser result:
   - With `kDNSServiceFlagsAdd`: record the PTR observation and `(instance, ifindex)`, then start `DNSServiceResolve` for that interface.
   - Without `kDNSServiceFlagsAdd`: drop that interface, stop its resolver, and send `SystemRemoved` once no interface is left. This is the same bookkeeping as avahi's scopes.
4. A resolve callback produces SRV (host, port) and TXT (split into strings) observations, and starts `DNSServiceGetAddrInfo(host, v4|v6)` on that interface if one is not already running. Each address callback with `kDNSServiceFlagsAdd` produces an A or AAAA observation.
5. All observations are `Source::System`, `ttl: None`. The interface name comes from `if_indextoname`, the same as avahi.
6. If an error callback or `DNSServiceProcessResult` returns `kDNSServiceErr_ServiceNotRunning`, the backend reports `Unavailable("mDNSResponder went away")` and returns. Other per-operation errors are logged at debug level and that operation is dropped.

**Threading:** `DNSServiceRef` and its callbacks are not `Send`, and the callbacks run inside `DNSServiceProcessResult`. The backend therefore runs its whole loop in `tokio::task::spawn_blocking`:

- `poll()` on the shared fd, with a 250 ms timeout so the loop can notice a closed channel.
- `DNSServiceProcessResult` handles what arrived.
- Callbacks push into a `Vec<Observation>` reached through the context pointer. The loop drains it with `observations.blocking_send`.
- The returned `BoxFuture` awaits the join handle.

This keeps every `unsafe` pointer on one thread. Each `DNSServiceRef` sits in a small RAII wrapper that calls `DNSServiceRefDeallocate` on drop. The shared connection is dropped last.

The backend name is `"mDNSResponder"`.

*Alternatives considered:*
- Crates such as `astro-dnssd` or `zeroconf`. They are either unmaintained or do not expose interface indexes or shared connections, and they add dependencies.
- Running `dns-sd -B` / `-L` as child processes and parsing their output. That is fragile and the output is meant for people.
- Using `DNSServiceQueryRecord` for raw records with TTLs. Not chosen, to keep the same daemon semantics as avahi (`ttl: None`, valid until removed). The wire source already supplies TTLs.

### 3. One default exclude list for all platforms

`InterfaceConfig::default().exclude` becomes `["veth*", "awdl*", "llw*", "utun*", "anpi*"]` on every platform. These names do not occur on Linux, so a single list keeps the spec, the README and the tests simple. Bridge members without addresses (`en1`–`en3`) need no pattern: joins skip them for IPv4, and IPv6 join failures are already logged and ignored.

*Alternative considered:* a `cfg`-dependent default. Rejected, because the documented default would then depend on the platform for no benefit.

### 4. Local Network permission detection

On macOS 15 and later, a process that the Local Network privacy setting applies to, and whose access has been denied, gets `EHOSTUNREACH` from multicast `send_to`. It also receives nothing.

Checking by hand (task 5.2) showed that the setting is not applied to programs started from a terminal. Denying Ghostty's access and restarting it changed nothing: multicast and even unicast TCP to LAN hosts kept working. The detection is therefore a safeguard, for a start outside a terminal or a stricter future macOS. It is covered by unit tests only.

`WireBackend::send_questions` will count the results of each round. If every attempted send failed with `EHOSTUNREACH`, and at least one was attempted, it emits:

`SourceStatus { Wire, Unavailable("blocked by macOS Local Network permission: allow the app that starts mdnsview in System Settings › Privacy & Security › Local Network") }`

The next round in which any send succeeds restores `Active`. This logic is behind `cfg(target_os = "macos")`. On Linux, `EHOSTUNREACH` has other causes and the current debug-level logging stays.

The store and status bar already handle a wire source becoming unavailable at runtime. Startup is not changed: the startup check only covers bind failures. A blocked permission therefore shows up in the status bar, not as an exit.

### 5. Paths and dependencies

- **Paths:** no code change. On macOS, `directories` 6 returns `~/Library/Application Support/mdnsview` for `config_dir` and `None` for `state_dir`, so logs fall back to `cache_dir` = `~/Library/Caches/mdnsview`. Only the spec and README change.
- **Dependencies:** `zbus` moves to `[target.'cfg(target_os = "linux")'.dependencies]`, so macOS builds don't compile the D-Bus stack. `NullBackend`'s `#[cfg_attr(target_os = "linux", allow(dead_code))]` changes to cover macOS too.

## Risks / Trade-offs

- [`ip_mreqn` or `IP_PKTINFO` behaves differently on Darwin at runtime] → Both are checked first, with the fallbacks from Decision 1 ready. A packet with an unknown ifindex is silently dropped by `handle_packet`, so the symptom would be "wire active, no records". The manual check looks for exactly that.
- [The FFI declarations are wrong] → They are copied exactly from the macOS SDK's `dns_sd.h`, including `DNSSD_API` = default C ABI on Apple and the `uint16_t` port in network byte order. Unit tests cover the pure helpers: TXT splitting, name assembly and port byte order. One `#[ignore]` live test runs against the real daemon, like `live_avahi`.
- [`blocking_send` stalls the dns_sd thread while the store is busy] → This is acceptable, because it is the same backpressure the avahi task gets from `send().await`. mDNSResponder buffers replies on its socket.
- [EHOSTUNREACH is a heuristic for the permission] → It is limited to macOS and only applies when every send in a round fails. The wording says "blocked by … permission", which is the only common cause there.
- [The Local Network safeguard cannot be exercised from a terminal, so it has no end-to-end test] → The decision function and the status-bar text are unit-tested. The README says plainly that the setting does not apply when started from a terminal.
- [The longer default exclude list is a behavior change for Linux users] → It only affects interfaces with those names, which Linux does not create by default. Users with an explicit `exclude` are not affected.

## Migration Plan

None needed. Existing Linux configs keep working unchanged. To roll back, revert the change. No data formats are involved.
