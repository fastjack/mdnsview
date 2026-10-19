# Tasks

## 1. Project setup

- [x] 1.1 Create the Cargo binary crate `mdnsview` with the module skeleton from design D10 (`config`, `model`, `backend`, `wire`, `ui`); verify `cargo build` and `cargo clippy -- -D warnings` succeed
- [x] 1.2 Add dependencies:
  - tokio, ratatui, crossterm (event-stream)
  - zbus (tokio), socket2, nix (net, socket, uio)
  - simple-dns
  - serde, toml, humantime-serde, directories, globset, clap
  - rand, tracing

  Verify `cargo build` resolves them.
- [x] 1.3 Add a `tracing` subscriber that writes to a log file in the platform state/cache directory, never to the terminal; verify a debug line appears in the log file when running with `RUST_LOG=debug`

## 2. Configuration

- [x] 2.1 Implement the `Config` struct with defaults, `deny_unknown_fields`, a humantime `device_timeout`, `interfaces.include`/`exclude` and `ipv6`; verify unit tests cover the defaults, a partial file, a bad duration and an unknown key (app-configuration spec scenarios)
- [x] 2.2 Implement path resolution: `ProjectDirs` default, `--config` override, missing file → defaults; parse errors name the file and the setting and exit non-zero before the UI starts. Verify unit tests and a manual run with a broken file.
- [x] 2.3 Document the configuration options with an example `config.toml` in README.md; verify the example parses in a unit test that loads it

## 3. Record model and store

- [x] 3.1 Define `Record`, `RecordKey`, `RData` (decoded and unknown-raw), `Source`, `Observation`, and the metadata fields from design D6; verify the code compiles and has unit tests for key normalisation (case-insensitive names)
- [x] 3.2 Implement store ingestion: insert/renew, source-set merge (provenance "both"), TTL expiry instant, goodbye on TTL 0, and the cache-flush rule from RFC 6762 §10.2. Verify paused-time tests for each case.
- [x] 3.3 Implement snapshot derivation:
  - devices by host name (SRV targets ∪ address owners)
  - host-only devices and the "unresolved" bucket
  - services and service types
  - the label order (device-info model → majority instance name → host name)

  Verify unit tests for every device-inventory grouping and label scenario.
- [x] 3.4 Add the built-in `service_names` table of readable type names (IPP, IPP Everywhere, printer, HTTP, SSH, SMB, AirPlay, RAOP, Google Cast, HomeKit, Spotify Connect, device-info, workstation, sftp, etc.); verify the known-type and unknown-type unit tests
- [x] 3.5 Implement the liveness states (live/stale/left), last-seen tracking (including the system-only "held by daemon counts as seen" rule), and GC by `device_timeout` that honours the retain set; verify paused-time tests for expiry, custom timeout, return to live, and retention
- [x] 3.6 Implement the store task: mpsc ingestion, `Retain` command, `QueryRequest` emission for entries without wire data, and `watch` snapshot publishing coalesced to ≤4/s; verify a tokio test that feeds observations and asserts on the published snapshots and query requests

## 4. Wire backend

- [x] 4.1 Implement `wire::codec`, which converts between simple-dns messages and `Record`/questions (cache-flush, unicast-response bit, unknown types kept raw); verify fixture-based tests under `tests/fixtures/` (an avahi response, an Apple device response, a goodbye, an unknown RR type)
- [x] 4.2 Implement `wire::interfaces`: getifaddrs enumeration, the up/multicast/non-loopback filter, the default `veth*` exclude, and include/exclude globs; verify unit tests for the mdns-discovery interface-selection scenarios using synthetic interface lists
- [x] 4.3 Implement `wire::sys`/`socket`:
  - v4/v6 sockets with SO_REUSEADDR/SO_REUSEPORT on 5353
  - per-interface join/leave
  - pktinfo `recvmsg` that yields the interface index
  - per-interface multicast send
  - IPv6 off via config

  Verify by running next to avahi-daemon as an unprivileged user and logging received packets with their interface.
- [x] 4.4 Implement the query scheduler (design D5): enumeration with exponential backoff and known-answer suppression; refinement requests with a 1 s dedup; TTL refresh at 80/85/90/95% with jitter, cancelled on renewal; 20 ms batching per interface. Verify paused-time tests that assert the emitted query times and contents.
- [x] 4.5 Wire the backend task together: receive loop → codec → `Observation`s, scheduler → send, 5 s interface poll with join/leave on changes, and a bind failure reported as unavailable; verify with `avahi-publish -s test _http._tcp 8080` that the service appears in the logged observations, and that disconnecting/reconnecting Wi-Fi resumes reception within 10 s

## 5. System backend

- [x] 5.1 Define the `SystemBackend` trait and `NullBackend`; verify the store task test runs with `NullBackend`
- [x] 5.2 Implement `AvahiBackend` over zbus:
  - signal match rules installed before the `*New` calls
  - type browser → service browser → resolver
  - `ItemRemove` → goodbye observation
  - unreachable daemon → reported unavailable

  Verify manually: `avahi-publish` services appear with host, port and TXT, and stopping the publish produces a removal.

## 6. Terminal UI

- [x] 6.1 Implement `App` state (focus, mode, filter, selection by stable ID, detail mode) and key handling (arrows/hjkl, tab, /, d, q); verify unit tests that feed key sequences and assert on the state
- [x] 6.2 Render the three-pane device mode: device rows with label + relative last seen, dimmed stale/left rows, alphabetical sort, stable selection; verify `TestBackend` tests, including "row inserted above selection keeps selection"
- [x] 6.3 Render the service-type pivot mode (types with readable names, instances annotated `@host`); verify `TestBackend` tests for the browse-by-type scenario
- [x] 6.4 Implement the `/` filter for both modes with the matching rules from the browser-ui spec, including moving the selection and releasing retention when the selected entry is hidden; verify unit tests for the find-printers and clear-filter scenarios
- [x] 6.5 Render the simple details view and the raw records view (remaining/original TTL, cache-flush, source, interface, provenance, first/last seen, hex for non-UTF-8 TXT, expired records marked); verify `TestBackend` tests for both views
- [x] 6.6 Implement selection retention: send `Retain` on selection/filter change, render retained entries as "gone Xm (pinned)", remove them after the selection moves away; verify a paused-time test with the store and `App` together
- [x] 6.7 Add the status bar (sources: "avahi + wire" / "wire only" / "system only", interface count, device count, key hint); verify `TestBackend` tests for each source combination
- [x] 6.8 Implement the UI loop (`select!` over events, snapshot changes and the 1 s tick) and the terminal restore guard with a panic hook; verify manually that `q` and a forced panic both leave the terminal usable

## 7. Integration

- [x] 7.1 Wire `main`:
  - config → backends → store → UI
  - degraded-mode selection
  - exit with an error naming both failures when no source starts

  Verify by running with avahi stopped ("wire only"), with port 5353 occupied without reuse ("system only"), and with both unavailable (error exit).
- [x] 7.2 End-to-end check on a real LAN:
  - devices appear within seconds
  - pivot, filter and raw view work
  - a device unplugged from the network becomes stale and is removed after the timeout (use `device_timeout = "30s"`)
  - a selected device is retained until the selection moves away

  Record the results in the change notes.
- [x] 7.3 Write a README section on usage, key bindings and the no-root/port-sharing behaviour; verify the documented commands run as written

## Workflow follow-up

- Archive the change after review with `/opsx:archive`.
