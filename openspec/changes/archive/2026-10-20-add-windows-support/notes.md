# Verification notes

Checked on 2026-10-20 on the development PC: Windows 11 Pro 23H2 (build 22631), x86_64, Rust 1.99.1 (`x86_64-pc-windows-msvc`), office LAN over Wi-Fi (`WLAN`, index 14, Public profile). The Windows UI language is German.

## Build scaffolding (tasks 1.1–1.3)

- **Dependencies.** `nix` is Unix-only now. `cargo tree -e normal -i nix` cannot answer on Windows: it reports that `nix` is ambiguous, because `Cargo.lock` holds `nix` 0.29 and 0.31 for other targets. `cargo tree -e normal | grep "nix v"` finds nothing. `Cargo.lock` still has one `windows-sys`, 0.61.2.
- **Linux.** After the `sys` split, `cargo clippy --all-targets --locked -- -D warnings` is clean in Docker `rust:latest` (aarch64 Linux).
- **Windows tests.** With the stub `sys::windows`, `cargo test` passes 127 tests, with 2 ignored: `live_wire` and `live_wire_ifindex`. One test assumed LF line endings: `readme_example_parses` looked for "```toml\n", but the Windows checkout has CRLF (`core.autocrlf=true`). It now skips to the end of the fence line.
- **Windows binary.** With the stub, `mdnsview.exe` exits with status 1 and names both failures (`multicast socket: … not implemented on Windows yet`, `none: no supported mDNS daemon`). That is the specified behavior when no source works.

## Interfaces (tasks 2.1, 2.2)

- **Live list.** `cargo test live_windows_interfaces -- --ignored --nocapture` lists:
  - `WLAN` (14, up, 192.168.4.83)
  - `Tailscale` (28, up)
  - `Bluetooth-Netzwerkverbindung 2` (12, down)
  - `Loopback Pseudo-Interface 1` (1, loopback)
  - two Hyper-V switches, `vEthernet (Default Switch)` (23) and `vEthernet (WSL (Hyper-V firewall))` (32), both up and multicast-capable
- **Hidden adapters.** The Hyper-V switches do not show in plain `Get-NetAdapter`, only with `-IncludeHidden`. They are selected by default, like `docker0` on Linux. The README will show `exclude = ["vEthernet (*"]` for users who want them skipped.
- **Selector tests.** "Default selection on Windows" and the `vEthernet (*` exclude pass.

## Multicast socket (tasks 3.1, 3.2)

- **Port sharing works on both families.** `open_v4` and `open_v6` both bind 5353 next to the DNS Client service (`svchost` on `[::]:5353`) and Edge (`0.0.0.0:5353`). The IPv6 limitation feared in the design does not occur.
- **Packet info works.** `cargo test live_wire_ifindex -- --ignored --nocapture` passed. Packets from 9 other hosts on IPv4 and 11 on IPv6 all reported `ifindex 14` (`WLAN`). The index join works, so the address-join fallback was not needed.
- **Parser tests.** `ipv4_pktinfo`, `ipv6_pktinfo_after_other_message` and `no_pktinfo` cover the control-message parser.

## Windows Firewall, measured (input for group 5)

- **Block rules, not "no rule".** During the first socket test, Windows created two inbound **Block** rules (Private, Public) for the test binary `target\debug\deps\mdnsview-….exe`. That is what happens when the firewall prompt is dismissed or cancelled. The WLAN connection is on the Public profile. "Unicast response to multicast" is enabled, which is the default. The shell is not elevated, so no allow rule could be added for comparison.
- **Listening only.** With a socket that only listens and sends nothing, 0 packets from other hosts arrived in 30 s.
- **After one query.** After a single `_services._dns-sd._udp` query, 24–32 packets arrived in 20 s:
  - most within 10–220 ms (the replies)
  - then individual packets for up to about 19 s, from hosts that had answered
- **Full wire backend.** `live_wire` ran for 150 s with `MDNSVIEW_LIVE_SECS=150`. Every 15-second bucket had 105–231 records from 13–27 other hosts, with no gap.
- **Conclusion.** Windows Firewall lets traffic in after mdnsview's own multicast sends, even with a Block rule. Unsolicited announcements are dropped. Because mdnsview keeps sending queries, the wire source never goes silent for 15 s on this PC, so the hint designed in Decision 5 would not appear here.

## Windows DNS-SD API spike (task 4.1)

Run with `cargo test live_windns_raw -- --ignored --nocapture` (40 s, `_ipp._tcp.local`), plus a probe that announced a made-up service `probe one._mdnsviewprobe._tcp.local` from our own socket and then sent its goodbye.

- **API present.** On build 22631, all six functions resolve with `GetProcAddress`. `DnsServiceBrowse` and `DnsServiceResolve` return `DNS_REQUEST_PENDING` (9506).
- **Service types.** The `_services._dns-sd._udp.local` browse lists 26 types within 0.1 s, with each type as a PTR target such as `_ipp._tcp.local`.
- **A browse keeps reporting until cancelled.**
  - Callbacks arrived at 0, 1, 3, 7, 15 and 31 s, following the query backoff.
  - `DnsServiceBrowseCancel` causes one last callback with status 1223 (`ERROR_CANCELLED`) and no records. So a browse context can be freed in that callback.
- **Browse results carry more than PTR.**
  - An instance's callback often also holds its SRV, TXT and A records, with real TTLs.
  - The TXT strings are exact (`["k=v", "flag"]`), so for TXT the browse records are better than the key/value form in `DNS_SERVICE_INSTANCE`.
- **Removals arrive.** After the probe's goodbye, the browse called back twice (IPv4 and IPv6 packet) with the instance's PTR, SRV, TXT and A records, all at "TTL" 0. Later runs showed that this field is not a TTL (see task 4.4 below). It read 0 here only because the DNS Client's clock was near zero. The fallback (missing from two rounds) is still not needed.
- **Resolve.**
  - Answers give host, port, `dwInterfaceIndex` 14 (`WLAN`), the TXT properties, and IPv4 and/or IPv6.
  - Only 5 of 11 resolves answered within 40 s. The others never called back, so the backend must not depend on resolve.
- **Consequence for the backend.**
  - Browse records are the main source: PTR, SRV, TXT, A and AAAA.
  - Resolve adds the interface name and fills in SRV and addresses. Its TXT is used only for instances whose browse delivered no TXT.
  - The spike tests were removed again; this section keeps their results.

## Shared name helpers (task 4.2)

- **Moved.** `instance_name`, `domain_or_local` and `parse_presentation` moved from `src/backend/dnssd.rs` to `src/backend/names.rs`, with `instance_names_keep_dots_in_the_instance_label` and `presentation_names_are_unescaped`. Both tests pass on Windows.
- **Not checked on macOS.** The `aarch64-apple-darwin` target is not installed on the PC, so `dnssd.rs` was not compiled after the move. The change is mechanical (imports from `super::names`), but it still needs `cargo test` on the Mac (task 8.1).

## Windows DNS backend (task 4.4)

- **The record "TTL" is an expiry time.** `DNS_RECORD.dwTtl` in browse results is the DNS Client's internal clock in seconds plus the record's TTL. Measured with `live_windns_goodbye` and a fresh test instance:
  - announced with TTL 120: the field read 4999
  - re-announced 2.2 s later with TTL 600: 5481
  - goodbye 3 s after that: 4884

  So the base was about 4879, 4881 and 4884 s, advancing with real time. In the first spike the clock was still near 0, so the goodbye read 0. The clock does not reset per browse or per run.
- **Goodbye detection.** The tracker therefore remembers each record's last expiry. A record whose expiry is 0, or drops by more than 1 s, counts as a goodbye. For a PTR, the instance is removed and its resolver stopped. Other goodbye records are skipped. Unit tests: `expiry_dropping_to_now_is_a_goodbye` and `goodbye_records_besides_the_ptr_are_skipped`.
- **Live tests.** Both passed twice in a row.
  - `live_windns`: within 4 s, 419 PTR, 174 SRV, 156 TXT, 166 A and 108 AAAA records. 310 records carried interface `WLAN` (learned from resolves); the others came from browse results before any resolve answered.
  - `live_windns_goodbye`: the instance `Probe 1.2 <pid>._mdnsviewtest._tcp.local` was reported with its dot inside the label, and was removed about 2 s after its goodbye.
- **Fresh name per run.** The goodbye test uses a new instance name each run. The DNS Client keeps earlier runs' names cached with their own expiry, and with a reused name the test was unreliable.
- **Checks.** `cargo clippy --all-targets -- -D warnings` is clean on Windows. `cargo test` passes 157 tests, with 5 ignored.

## Firewall hint (tasks 5.1, 5.2)

- **Status bar.** `Status` has two new fields: `wire_silent`, from `Observation::WireSilent`, and `system_services`, which is set when a snapshot is derived (the daemon holds a PTR to a service instance). The test `firewall_hint` covers four cases: no hint without system instances, the hint, no hint while the daemon is unavailable, and the hint cleared.
- **Detection.** `silence(first_send, last_foreign, now)` has its own tests. The wire backend checks it on the 5-second interface poll and reports changes only on Windows (`cfg!(windows)`, like the macOS `cfg!` check).
- **On the PC.** A 30 s `live_wire` run sent no `WireSilent`, as expected, since replies get through (see the firewall measurements above).

## Paths (tasks 6.1, 6.2)

- **Config path.** No `%APPDATA%\mdnsview` existed on the PC. A temporary `%APPDATA%\mdnsview\config\config.toml` containing `device_timeout = "five minutes"` made `cargo run` exit with status 1 and the error `invalid configuration in C:\Users\micro\AppData\Roaming\mdnsview\config\config.toml: …`. The folder was removed afterwards.
- **Log path.** The log was written to `C:\Users\micro\AppData\Local\mdnsview\cache\mdnsview.log` (`%LOCALAPPDATA%\mdnsview\cache`), as design Decision 6 expects. It was written by the stub run during task 1.3, at the default `info` level. The folders match `directories` 6, so the spec delta needs no change.
- **README.** The configuration and log paragraphs now name the Windows paths.

## README (task 7.2)

- **New section.** "Windows" covers the firewall behavior and allow rule, interface names with an exclude example, and the config and log paths. Usage mentions `.\target\release\mdnsview.exe`. There is no IPv6 limitation to document (task 3.1).
- **Test change.** `readme_example_parses` now takes the first TOML block under "## Configuration", because the Windows section has its own TOML snippet.
- **Commands run on the PC:** `Get-NetAdapter -IncludeHidden`, both paths, and the exclude pattern (unit test `windows_names_in_patterns`).
- **Not run: the `New-NetFirewallRule` command.** It needs an elevated shell, so it is part of the manual check in task 5.3.

## Integration (task 8.1, Windows and Linux parts)

- **Windows 11 23H2 (build 22631), x86_64, Rust 1.99.1.** `cargo clippy --all-targets -- -D warnings` is clean. `cargo test` passes 159 tests, with 5 ignored: `live_windns`, `live_windns_goodbye`, `live_wire`, `live_wire_ifindex` and `live_windows_interfaces`. All five ignored tests passed when run by hand during this change. `cargo build --release` succeeds.
- **Linux (Docker `rust:latest`, Rust 1.99.1, aarch64).** `cargo clippy --all-targets --locked -- -D warnings` is clean. `cargo test --locked` passes 132 tests, with 3 ignored: `live_avahi`, `live_wire` and `live_wire_ifindex`.
- **macOS (user, on a separate Mac, branch `add-windows-support` at `0979bcf`).** `cargo test` passes and `cargo clippy --all-targets -- -D warnings` is clean, so the `names.rs` move (task 4.2) also builds there.

## Manual checks (tasks 3.3, 4.5, 5.3, 7.1)

- **App run (user).** Windows Terminal, release build: the status bar reads `Windows DNS + wire`, and the printers' records show `via both` in the raw records view.
- **`.local` resolution.** `Resolve-DnsName <host>.local -LlmnrNetbiosOnly` resolves `KMC15790.local` (192.168.4.129) and `BRN30055CF27350.local` (192.168.4.102) while mdnsview runs, the same as without it. On this PC the plain `Resolve-DnsName` and `ping` fail for `.local` with or without mdnsview, because the company DNS server answers NXDOMAIN first.
- **Daemon unavailable.** Stopping Dnscache needs administrator rights, so it was not done. The unavailable path is covered by `missing_api_is_reported_unavailable` (backend) and the `wire only` label tests (UI).
- **Firewall (user).** After 60 s without an allow rule, no firewall hint appeared. The user then added the README's `New-NetFirewallRule` command in an elevated PowerShell and deleted the two Block rules. `Get-NetFirewallApplicationFilter` now shows only Allow rules for `target\release\mdnsview.exe`. The hint itself cannot be triggered here; unit tests cover it.
- **UI (user).** In Windows Terminal, keys, mouse, `m`, colors, box drawing and quitting all work. The classic console was not checked; the user reports it is no longer available on this Windows 11.
- **macOS.** Checked by the user after the push; see the integration section above.
