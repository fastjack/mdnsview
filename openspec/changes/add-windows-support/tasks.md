# Tasks

## 1. Platform split and build scaffolding

- [x] 1.1 Move `nix` to `[target.'cfg(unix)'.dependencies]` and add `windows-sys = "0.61"` under `[target.'cfg(windows)'.dependencies]` with the features from design Decision 6. Widen `NullBackend`'s dead-code `cfg_attr` to include Windows. Verify that `cargo tree -e normal -i nix` on Windows reports no match, and that `Cargo.lock` still has a single `windows-sys` 0.61.2.
- [x] 1.2 Turn `src/wire/sys.rs` into `src/wire/sys/{mod.rs, unix.rs}` as in design Decision 1:
  - the shared constants and `Received` live in `mod.rs`
  - the Unix code, unchanged, lives in `unix.rs` behind `#[cfg(unix)]`
  - `live_wire_ifindex` moves to `mod.rs`, with the `WLAN` default on Windows

  Add a `windows.rs` stub with the same public API, whose functions return `io::ErrorKind::Unsupported`. Verify that `cargo check` succeeds on Windows except for `clock`, and that the Linux build still passes `cargo clippy --all-targets -- -D warnings` (Docker `rust:latest`, as in the macOS change).
- [x] 1.3 Make `clock` in `src/ui/view.rs` use `localtime_s` on Windows (design Decision 6). Leave the `local_network_blocked` call site with its runtime `cfg!` (design Decision 6). Verify that `cargo build` succeeds on Windows, and that `cargo test` passes on Windows. The stub backend makes the binary run as "no sources". Fix any test that assumes Unix paths, and record the test count in `notes.md`.

## 2. Windows interfaces

- [x] 2.1 Implement `list_interfaces` in `src/wire/sys/windows.rs` with `GetAdaptersAddresses` and the field mapping from design Decision 3. Put the mapping from one adapter entry to an `Interface` in a pure function. Verify:
  - unit tests of that function cover a connected Wi-Fi, a disconnected adapter, the loopback pseudo-interface, a no-multicast adapter and an IPv6-only adapter (`IfIndex == 0`)
  - an `#[ignore]` test `live_windows_interfaces` prints the list, and on the dev PC it shows `WLAN` (14) and `Tailscale` (28) as up
- [x] 2.2 Add the selector test "Default selection on Windows" to `src/wire/interfaces.rs`: `WLAN` connected, `Bluetooth Network Connection` down, `Loopback Pseudo-Interface 1` loopback → only `WLAN`. Add a test that `exclude = ["vEthernet (*"]` drops `vEthernet (WSL)`. Verify with `cargo test`.

## 3. Windows multicast socket

- [x] 3.1 Implement `MdnsSocket::open_v4` / `open_v6` and `share_port` per design Decision 2: `SO_REUSEADDR`, `IP_PKTINFO` / `IPV6_PKTINFO`, `SIO_UDP_CONNRESET` off, and a tokio `UdpSocket`. Verify that both open without error on the dev PC while the DNS Client and Edge hold 5353. If `open_v6` fails, record the error in `notes.md`, and in the README task (7.2) document that Windows runs IPv4-only on the wire.
- [x] 3.2 Implement `join`, `leave` and `send_to_group`. Implement `recv` with `async_io` + `WSARecvMsg`, fetching the function pointer once per socket. Parse the control messages in a pure function. Verify:
  - a unit test of the parser over hand-built `IN_PKTINFO` and `IN6_PKTINFO` control buffers
  - `cargo test live_wire_ifindex -- --ignored --nocapture` on the PC, after allowing `mdnsview`'s test binary through the firewall, receives packets from other hosts with `ifindex == 14` on IPv4 and, if 3.1 succeeded, on IPv6

  If the index join receives nothing, switch to the address join behind `cfg(windows)` and rerun the test.
- [x] 3.3 Run the release binary on the PC (no allow rule needed, see notes.md). Verify:
  - the status bar shows `none + wire` with the LAN's devices
  - `ping <some-host>.local` still resolves while `mdnsview` runs
  - Edge still runs normally

  Record the result in `notes.md`.

## 4. Windows DNS backend

- [x] 4.1 Spike: in an `#[ignore]` test `live_windns_raw`, resolve the API at runtime, browse `_services._dns-sd._udp.local` and one type found on the LAN for 30 s, and print every callback: status, record type, name and TTL. Record in `notes.md`:
  - whether a browse keeps reporting or completes on its own
  - whether TTL-0 PTRs arrive when a service goes away (stop a service, for example a phone's AirPlay)
  - what `dwInterfaceIndex` the resolve reports

  Pick the browse-lifetime and removal strategy from design Decision 4 based on these results.
- [x] 4.2 Move `parse_presentation` and `instance_name` from `src/backend/dnssd.rs` to `src/backend/names.rs` (`cfg(any(target_os = "macos", windows))`), together with their tests. Verify that `cargo test` passes on Windows with the moved tests. The macOS build cannot be checked here, so confirm with `cargo check --target aarch64-apple-darwin` if the target is installed, or note it for the reviewer.
- [x] 4.3 Implement the pure `Tracker` for Windows in `src/backend/windns.rs`, following design Decision 4 as revised after 4.1:
  - browse records (PTR, SRV, TXT, A, AAAA) to observations, and resolve starts
  - resolve results to SRV and A/AAAA observations, plus TXT only when the instance's browse gave none
  - goodbye PTRs to `SystemRemoved`
  - TXT key/value rebuilding

  Verify with unit tests over synthetic inputs:
  - a meta-PTR starts one type browse, and only one
  - a PTR starts a resolve
  - a goodbye PTR (expiry 0 or dropped, see design Decision 4) sends `SystemRemoved` and stops the resolver
  - browse TXT wins over resolve TXT
  - the resolve's interface is applied to later browse records of that instance
  - TXT `key=value`, `key` with an empty value and `key` with a null value
  - an instance name like `Mr. Printer`
- [x] 4.4 Wire up the FFI:
  - runtime loading of the six functions
  - one boxed `OpContext` per operation, freed in its final callback (`ERROR_CANCELLED` or the resolve completion)
  - the driver task that owns the tracker and the cancel handles, and cancels everything when the channel closes
  - `DnsServiceFreeInstance` / `DnsFree` on every callback
  - `Active`/`Unavailable` reporting as in design Decision 4

  Name the backend `"Windows DNS"` and select it in `backend::platform()` on Windows. Verify:
  - an `#[ignore]` test `live_windns`, modeled on `live_dnssd`, prints at least one SRV, TXT and A/AAAA record within 4 s
  - an `#[ignore]` test `live_windns_goodbye` announces a made-up service from our own socket, then sends its goodbye, and the backend reports it and then sends `SystemRemoved` (as the 4.1 probe did)
  - a unit test of the missing-function path gives `Unavailable` with the API message
- [x] 4.5 Run the app on the PC without the firewall rule. Verify that the status bar starts with `Windows DNS + wire`, and that the LAN printers appear with host, port, TXT and addresses through `via system`. Then stop Dnscache, if that is allowed on this PC; otherwise simulate the failure with a forced error in a debug build. Check that the status bar shows `wire only`. Record both in `notes.md`.
- [x] 4.6 Document the Windows source in the README section "How discovery works": "the DNS Client service on Windows, through the Windows DNS-SD API". Verify that the status-bar names in the README match the backend's `name()`.

## 5. Firewall hint

- [x] 5.1 Add `Observation::WireSilent(bool)` and `Status::wire_silent`, and implement the label rule from design Decision 5 in `Status::sources_label`. Verify with tests in `src/ui/tests.rs`:
  - hint shown: silent, system active, one system instance
  - no hint: silent but no system instances (spec scenario "Empty network")
  - hint cleared after `WireSilent(false)`, with the exact strings from the `browser-ui` delta
- [x] 5.2 In `WireBackend` (emitting only on Windows): track own addresses in `sync_interfaces` and `last_foreign` in `handle_packet`, and emit `WireSilent` changes on the interface poll ticks, once per change. Put the decision in a pure function `silence(first_send, last_foreign, now) -> bool`. Verify with unit tests: before 15 s, after 15 s without foreign traffic, foreign traffic at 10 s, and silence returning 15 s after the last foreign packet.
- [x] 5.3 On the PC, run the app without an allow rule for at least 60 s. Verify that no hint appears, since replies get through (spec scenario "Default firewall without an allow rule"). Then, in an elevated PowerShell, run the following:

  ```powershell
  New-NetFirewallRule -DisplayName "mdnsview (mDNS-In)" -Direction Inbound -Protocol UDP -LocalPort 5353 -Program <path to mdnsview.exe> -Action Allow
  ```

  Verify that unsolicited announcements now arrive: a listen-only socket receives packets from other hosts within 30 s. The hint itself cannot be triggered here; the unit tests of 5.1 and 5.2 cover it. Record the result in `notes.md`.

## 6. Paths

- [x] 6.1 Add the spec scenario's location to the config docs. Verify on the PC: `cargo run` with an invalid `%APPDATA%\mdnsview\config\config.toml` exits with an error naming exactly that path. Remove the file afterwards.
- [x] 6.2 Verify that the log is written to `%LOCALAPPDATA%\mdnsview\cache\mdnsview.log` with `RUST_LOG=mdnsview=debug`. If `directories` returns different folders than design Decision 6 states, update the `app-configuration` delta and the design before the README task.

## 7. README

- [x] 7.1 Check the UI by hand in Windows Terminal and in the classic console (conhost):
  - keys act once per press
  - mouse click and wheel work, and `m` toggles capture
  - colors and box-drawing characters render
  - `q` and `ctrl-c` restore the terminal

  Record the result in `notes.md`. Fix any issue that shows up, with a test where the logic allows one.
- [x] 7.2 Add a "Windows" section to the README:
  - the firewall rule command and why it is needed
  - interface names (`Get-NetAdapter`) and an example exclude for `vEthernet (*` and `Tailscale`
  - the config and log paths
  - the IPv6 limitation, if task 3.1 found one

  Update the usage text for `mdnsview.exe` and the platform list in "How discovery works". Verify that the README example parse test still passes, and that every path and command in the section was run on the PC.

## 8. Integration

- [ ] 8.1 On Windows, run `cargo clippy --all-targets -- -D warnings` and `cargo test`. On Linux (Docker `rust:latest`), run `cargo clippy --all-targets --locked -- -D warnings` and `cargo test --locked`. If an Apple target is installed, also run `cargo check --target aarch64-apple-darwin`; otherwise ask the user to run `cargo test` on the Mac. Record the test counts per platform and the Windows build in `notes.md`.
- [x] 8.2 Run `openspec validate add-windows-support --strict` and verify that it passes.

## Workflow follow-up

- Archive the change after review with `/opsx:archive`.
