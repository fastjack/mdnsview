# Verification notes

Checked on 2026-10-20 on the development machine (macOS 27.2, build 26B118, arm64, Rust 1.99.0 from Homebrew) on the office LAN over Wi-Fi (en0, 192.168.4.78/24).

## Wire socket (tasks 2.1, 2.2)

- **IPv4 join by index does not work on macOS.**
  - With `ip_mreqn` by interface index, `live_wire_ifindex` received nothing on IPv4 in 3 s, not even our own looped-back query.
  - Joining by the interface's IPv4 address (`v4_membership` in `src/wire/sys.rs`) fixed it.
  - IPv6 join by index works as before.
- **Packet info works.** `IP_PKTINFO` and `IPV6_RECVPKTINFO` report the receiving interface on macOS. Every packet in the test had ifindex 14 (en0). The `IP_RECVIF` fallback was not needed.
- **Live test results.** `cargo test live_wire_ifindex -- --ignored --nocapture` passed three times in a row. Each run saw packets from 15–20 other hosts on both IPv4 and IPv6, all on en0.
- **One unexplained empty run.** Right after the address-join fix, one run received no IPv4 packets in 3 s, our own query included. It did not happen again in later runs.

## Alongside mDNSResponder (task 2.3)

- **Bonjour unaffected.** The release build ran in tmux (160×40). Meanwhile, `dns-sd -B _airplay._tcp local.` in another shell listed AirPlay devices as usual, and `dns-sd -B _services._dns-sd._udp` worked too. Finder was not checked, because this was done from a shell session only.
- **Wire records captured.** mdnsview showed 21 devices from wire records: Brother and Kyocera printers, Macs and signage. Each record had its source address and `if en0`.
- **Status bar.** It read `wire only │ 24 interfaces`. At this point the null backend was still in use and the default excludes were not yet updated.

## mDNSResponder backend (tasks 4.1–4.5)

- **Live test.** `cargo test live_dnssd -- --ignored --nocapture` passed. Within 4 s it reported SRV, TXT and A/AAAA records:
  - LAN devices on en0: Brother and Kyocera printers, TeamViewer hosts, iPhones and Macs
  - the Mac's own services on lo0
- **App status bar.** It reads `mDNSResponder + wire │ 9 interfaces │ 20 devices`.
- **Refinement of daemon results.** Checked on Brother DCP-L8400CDN in the raw records view:
  - PTR, SRV and A were `via both`, with source 192.168.4.102 on en0.
  - TXT stayed `via system`.
- **Why the TXT stays system-only.** The debug log shows this cause; the printer's behavior is a guess.
  - mDNSResponder answers from its warm cache, so about 150 refinement questions go out within ~100 ms of startup.
  - The printer answered SRV and A but apparently not TXT.
  - Because a wire SRV satisfies the store's refinement check (`refinements` in `src/model/store.rs`), TXT is never asked for again.
  - This is existing store behavior and not specific to macOS. It is left for a follow-up.
- **Expected warnings.** IPv6 group joins fail with EINVAL on en1–en3, the Thunderbolt bridge members that have no addresses. They are logged as warnings and ignored, as the design expects.

## Local Network permission (task 5.1)

- **Detection.** `local_network_blocked` in `src/wire/backend.rs` reports blocked only when every send in a round fails with `EHOSTUNREACH`. Unit tests cover all-unreachable, mixed, other errors, all-ok and empty rounds.
- **Status bar change.** The task list did not mention this; the spec needs it. The status bar never showed a source's reason before, so `Status::sources_label` now adds `(Local Network access blocked)` when the wire source reports that reason. The reason text is the shared constant `LOCAL_NETWORK_BLOCKED`. `status_bar_sources` covers blocked and recovered.

## Paths and README (task 5.3)

- **Config path.** With `HOME` pointed at a scratch directory containing an invalid `Library/Application Support/mdnsview/config.toml`, mdnsview exited with an error naming exactly that file.
- **Log path.** The log was written to `~/Library/Caches/mdnsview/mdnsview.log`.

## Integration (task 6.1, macOS part)

- **macOS.** On macOS 27.2 (arm64), `cargo clippy --all-targets -- -D warnings` is clean, and `cargo test` passes 122 tests with 3 ignored live tests: `live_wire`, `live_wire_ifindex` and `live_dnssd`.
- **Formatting.** `cargo fmt --check` already reported differences before this change, so formatting was left as is.
- **Linux.** Checked in Docker with the official `rust:latest` image (Rust 1.99.1, aarch64 Linux), with the source mounted read-only.
  - `cargo clippy --all-targets --locked -- -D warnings` is clean.
  - `cargo test --locked` passes 113 tests with 3 ignored: `live_avahi`, `live_wire` and `live_wire_ifindex`.
  - Before this change, Linux had 111 passing and 2 ignored. The two new passing tests are the macOS interface selection test and `local_network_blocked`. The new ignored test is `live_wire_ifindex`.

## Local Network permission by hand (task 5.2)

- **Setup.** On 2026-10-20 the user denied Local Network access for Ghostty in System Settings and restarted Ghostty. The checks below ran in a session under that Ghostty: `ghostty → login → zsh → claude`.
- **mdnsview still works.** The user saw the same devices as before.
- **The multicast socket still works.** `cargo test live_wire_ifindex -- --ignored` passed. Packets from other LAN hosts arrived on IPv4 and IPv6, and sends did not fail.
- **Plain unicast still works too.** TCP connects from a non-Apple binary (pyenv `python3`) to 192.168.4.102:631 and 192.168.4.123:80 succeeded.
- **Conclusion.** In this setup, macOS does not enforce the denial for command-line programs started from the terminal. mdnsview never gets `EHOSTUNREACH`, so the blocked state in the status bar cannot be triggered by hand here.
- **Decision.** The user chose to keep the detection as a safeguard. The spec scenario, the design, the proposal and the README now say that the setting does not apply when mdnsview is started from a terminal. The reason text now says "allow the app that starts mdnsview" instead of "allow your terminal".
