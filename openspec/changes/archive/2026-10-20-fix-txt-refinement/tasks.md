# Tasks

## 1. Per-type refinement

- [x] 1.1 In `src/model/store.rs`, change `refinements` to compute the missing types:
  - instance: SRV and TXT, each missing unless that type is on the wire
  - host: `[A, AAAA]` until either is on the wire

  Request only the missing types. Update `refinement_requested_for_daemon_only_instances`: after a wire SRV arrives and 31 s pass, a re-ingested system PTR now requests `[TXT]`. Add a test, "SRV received without TXT": a wire SRV, then a system PTR for the same instance, requests `[TXT]` only. Add a test that a wire PTR followed by a wire TXT and a wire SRV requests nothing more. Verify with `cargo test`.

## 2. Bounded retries

- [x] 2.1 Replace `refine_requested` with the `refinements` table from design Decision 2: `Refinement { kind, requested, retries, next_retry }` with `RETRY_DELAYS = [2 s, 8 s, 30 s]`. Add `Store::due_refinements(now)`, and prune finished entries in `gc` after `REFINE_INTERVAL`. Add paused-time unit tests:
  - an unanswered instance is requested at 0 s, 2 s, 10 s and 40 s, and not again at 100 s
  - a TXT arriving after the first retry stops further requests, and the entry is gone at the next `due_refinements`
  - a host with only a wire A record is not retried
  - the 30 s throttle on re-ingest still holds while a round is running

  Verify with `cargo test`.
- [x] 2.2 In the store task (`run` in `src/model/store.rs`), call `due_refinements` on the GC tick and send the results on the query channel. Extend `store_task_publishes_snapshots_and_queries`, or add a task test, so that a retry for an unanswered instance arrives on the query channel after about 2 s of paused time. Verify with `cargo test`.

## 3. Integration

- [x] 3.1 Run `cargo clippy --all-targets -- -D warnings` and `cargo test` on macOS, then the same in Docker (`rust:latest`) for Linux. Record the test counts in `notes.md`.
- [x] 3.2 Run the release build on the office LAN. Verify that the Brother DCP-L8400CDN `_ipp` service's TXT shows `via both`, with source 192.168.4.102, within about 10 s of startup, and that the `_http` and `_scanner` services of the same printer have SRV and TXT `via both`. Check the debug log for the number of refinement retries sent in the first minute. Record both in `notes.md`.

## Workflow follow-up

- Archive the change after review with `/opsx:archive`.
