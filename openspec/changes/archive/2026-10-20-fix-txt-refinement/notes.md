# Verification notes

## Tests (task 3.1)

- **macOS.** On macOS 27.2 (arm64, Rust 1.99.0), `cargo clippy --all-targets -- -D warnings` is clean, and `cargo test` passes 144 tests with 3 ignored live tests. That is 137 before this change plus 7 new tests.
- **Linux.** In Docker `rust:latest` (Rust 1.99.1, aarch64), clippy is clean, and `cargo test --locked` passes 135 tests with 3 ignored. That is 128 before plus the same 7.
- **New tests:**
  - `srv_received_without_txt_requests_only_txt`
  - `fully_answered_instance_needs_no_refinement`
  - `unanswered_refinement_is_retried_with_backoff`: requests at 0, 2, 10 and 40 s, then none up to 100 s
  - `retries_ask_only_for_what_is_missing_and_stop_once_answered`
  - `host_with_only_an_a_record_is_not_retried`
  - `reingest_is_throttled_while_a_round_runs`
  - `store_task_sends_refinement_retries`: the retry arrives 2–3 s after the first request
- **Changed test.** In `refinement_requested_for_daemon_only_instances`, a re-ingested PTR after a wire SRV now asks for `[TXT]` instead of nothing.

## Office LAN (task 3.2)

The release build ran with `RUST_LOG=mdnsview=debug` in tmux (160×50), with mDNSResponder and the wire source both active.

- **Brother DCP-L8400CDN (192.168.4.102).** Before this change, its `_ipp` TXT showed `via system`. About 12 s after startup:
  - `_ipp`: PTR, SRV, TXT and A are all `via both`. The TXT has ttl 223/240 and cache-flush.
  - `_scanner`: SRV and TXT are both `via both`.
  - `_http`: SRV and TXT are both `via both`.
  - No retry was needed for the printer. The first request, which asked only for the missing TXT, was answered.
- **Retries in the first ~62 s.** There were 9, all of them first retries.
  - 6 were for `[SRV, TXT]` of Apple instances: `_apple-mobdev2`, `_remotepairing` and `_asquic` on iPhones and Macs.
  - 3 were for `[A, AAAA]` of hosts: `Ren.local`, `Stimpy.local` and an iPhone.
  - None needed a second retry, so each was answered after its first retry.
- **Added debug log line.** It was needed to count retries for this task: `due_refinements` logs `refinement retry N for <name>: <types>`.
- **Final check.** Clippy and the 144 macOS tests still pass with the log line.
