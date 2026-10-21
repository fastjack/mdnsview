# Tasks

## 1. Build on macOS

- [x] 1.1 Move `zbus` to `[target.'cfg(target_os = "linux")'.dependencies]`, put `set_multicast_all_v4` in `src/wire/sys.rs` behind `#[cfg(target_os = "linux")]`, and widen `NullBackend`'s dead-code `cfg_attr` so it no longer triggers on macOS. Verify that `cargo build` and `cargo clippy --all-targets -- -D warnings` succeed on macOS, and that `cargo tree -e normal | grep zbus` is empty there.
- [x] 1.2 Update the module comment in `src/wire/sys.rs` to say it covers Linux and macOS. Verify that `cargo test` passes on macOS: all non-ignored tests, unchanged in number.

## 2. Wire socket on macOS

- [x] 2.1 Check at runtime on macOS that IPv4 and IPv6 packets arrive with a non-zero `ifindex` that matches the joined interface. Use an `#[ignore]` live test `live_wire_ifindex` that opens both sockets, joins `en0`, sends one `_services._dns-sd._udp.local` query and asserts that a response is received with `ifindex == if_nametoindex("en0")`. If `ifindex` stays 0, implement the `IP_RECVIF` fallback from design Decision 1 and rerun the test.
- [x] 2.2 Check that the IPv4 `ip_mreqn` join by index delivers group traffic on macOS, using the same live test with IPv4 only. If it does not, switch the macOS join and leave to `InterfaceIndexOrAddress::Address(first v4)` behind `cfg(target_os = "macos")` and rerun the test.
- [x] 2.3 Run the release binary on the Mac while Bonjour is in use: `dns-sd -B _services._dns-sd._udp` in another terminal, and Finder showing network devices. Verify that both keep listing devices while `mdnsview` shows `wire` records with their interface names. Record the result in `notes.md`.

## 3. Default interface excludes

- [x] 3.1 Change `InterfaceConfig::default().exclude` to `["veth*", "awdl*", "llw*", "utun*", "anpi*"]`. Update `empty_file_gives_defaults` and add a selector test, "Default selection on macOS": lo0, en0, awdl0, llw0, utun3 and anpi0 select only en0. Verify with `cargo test`.
- [x] 3.2 Update the README configuration example and the interface text to show the new default exclude list. Verify that the README example parse test still passes and that its `exclude` value equals `InterfaceConfig::default().exclude`.

## 4. mDNSResponder backend

- [x] 4.1 Add `src/backend/dnssd.rs` (`#[cfg(target_os = "macos")]`). It contains the `extern "C"` declarations from `dns_sd.h`: `DNSServiceCreateConnection`, `DNSServiceRefSockFD`, `DNSServiceProcessResult`, `DNSServiceRefDeallocate`, `DNSServiceBrowse`, `DNSServiceResolve` and `DNSServiceGetAddrInfo`, plus the flag and error constants and the callback types. Add an RAII `ServiceRef` wrapper. Verify that it builds on macOS and that clippy is clean.
- [x] 4.2 Implement the pure helpers:
  - instance name assembly from a browse result (name, regtype, domain), including dots inside the instance label
  - the service-type name from a `_services._dns-sd._udp` result (`_ipp` + `_tcp.local.`)
  - TXT splitting into length-prefixed strings
  - port conversion from network byte order

  Verify with unit tests for each, including an empty TXT (a single zero byte) and an instance name like `Mr. Printer`.
- [x] 4.3 Implement the browse → resolve → get-address flow on one shared connection, following design Decision 2:
  - per-instance interface bookkeeping, and `SystemRemoved` once an instance's last interface is gone
  - resolvers stopped on removal
  - `SourceState::Active` sent after the connection succeeds, and `Unavailable` on `kDNSServiceErr_ServiceNotRunning`

  Verify with unit tests that drive the callback handling functions directly with synthetic inputs: add on two interfaces, then remove on one (no removal is sent), then remove on the other (`SystemRemoved` is sent).
- [x] 4.4 Run the loop in `spawn_blocking`, with `poll()` (250 ms timeout), `DNSServiceProcessResult`, and `blocking_send` of the collected observations. Stop when the channel closes. Name the backend `"mDNSResponder"`, and select it in `backend::platform()` on macOS. Verify with an `#[ignore]` test `live_dnssd`, modeled on `live_avahi`, that prints at least one SRV, TXT and A/AAAA record from the LAN within 4 seconds.
- [x] 4.5 Run the app on the Mac against a LAN with a printer or another Apple device. Verify that the status bar shows `mDNSResponder + wire`, and that a service known only from mDNSResponder gets wire metadata after the refinement queries. Record the result in `notes.md`.
- [x] 4.6 Document the macOS source in the README "How discovery works" section ("mDNSResponder on macOS, through the system `dns_sd` API"). Verify that the status-bar names in the README match the backend's `name()`.

## 5. Local Network permission

- [x] 5.1 Under `cfg(target_os = "macos")`, change `WireBackend::send_questions` to report whether every attempted send in the round failed with `EHOSTUNREACH`. The `run` loop then emits `SourceStatus { Wire, Unavailable("blocked by macOS Local Network permission: …") }`, and returns to `Active` on the next round with a successful send, sending each change only once. Verify with a unit test of the decision function over these lists of send results: all `EHOSTUNREACH`, mixed, empty, and all `Ok`.
- [x] 5.2 Deny Local Network access for the terminal in System Settings › Privacy & Security › Local Network, restart the terminal, then start `mdnsview`, and record in `notes.md` what happens to the wire source. Outcome: macOS did not apply the denial to programs started from the terminal; mdnsview, `live_wire_ifindex` and plain unicast TCP to LAN hosts all kept working. Per the user's decision the detection stays as an untriggerable-from-a-terminal safeguard; the spec, design, proposal and README were reworded to say so.
- [x] 5.3 Add a macOS section to the README:
  - how the Local Network setting relates to mdnsview, and where to change it
  - the config path `~/Library/Application Support/mdnsview/config.toml`
  - the log path `~/Library/Caches/mdnsview/mdnsview.log`

  Verify that the paths match `ProjectDirs` output on the Mac, using `cargo run -- --config /nonexistent` and the path named in the error message, or a debug log line.

## 6. Integration

- [x] 6.1 On macOS, run `cargo clippy --all-targets -- -D warnings` and `cargo test`. Then run `cargo check --target x86_64-unknown-linux-gnu`, or a Linux build on the dev machine, to confirm the Linux build still compiles with avahi. Record the test counts and the macOS version in `notes.md`.

## Workflow follow-up

- Archive the change after review with `/opsx:archive`.
