# Tasks

## 1. Derivation: one service per SRV target

- [x] 1.1 Add a model test helper that ingests a wire record with a given source address (the existing `rec` always uses 192.168.1.40). Add a `teamviewer_twins` fixture that feeds the observed records: the shared PTR, an SRV and TXT per PC from 192.168.4.135 and `fe80::c96a:fae5:472d:f9c`, and an A/AAAA record per host. Verify the fixture test compiles and, before the fix, reproduces one "0" device plus a service-less `DESKTOP-EAOAATI.local` device.
- [x] 1.2 In `snapshot::derive`, group an instance's SRV records by target and build one `Service` per target (design D1). Each service's own records are its SRVs, its TXTs and the shared PTR/other records. Compute host, addresses and port per service, and liveness, last-seen and `gone` from its life records only (SRVs and assigned TXTs). Verify:
  - the fixture yields two devices labelled "0", one `_teamviewer._tcp` service each, and no address-only device for either host
  - all existing model and UI tests pass unchanged
- [x] 1.3 Assign TXT records by `src_addr` against each target's address records, giving unassigned TXTs to targets without one of their own (D2). Verify model tests for "TXT assigned by source address" (each service shows its own `DyngateID`) and "TXT without a known source" (both services show it).
- [x] 1.4 Implement service ids (D3): the first target (earliest SRV `first_seen`, ties → smallest host key) keeps the instance key, and the other targets get `"{instance key}\u{0}{host key}"`. Verify model tests:
  - a single-target instance keeps its plain id
  - the first PC keeps the plain id when a twin appears
  - the remaining service takes the plain id after the first twin's records are removed
- [x] 1.5 Add `Service.shared_with` (the other targets, sorted) and verify a model test that each twin lists the other host, and an ordinary service lists none.

## 2. Store: cache-flush scope and GC of shared records

- [x] 2.1 Limit `Store::apply_cache_flush` for SRV to siblings with the same target, and for TXT to siblings with the same `src_addr`, still flushing siblings without one (D4). Verify model tests:
  - announcements from the two PCs a minute apart do not expire each other's SRV or TXT
  - a port change from the same host still expires the old SRV after 1 s
  - the existing address cache-flush tests pass unchanged
- [x] 2.2 Make `Store::gc` keep records that are also own keys of a service it keeps (D4). Verify model tests:
  - "One PC goes silent": the silent host's service is removed, while the PTR and the other service remain
  - "One PC leaves": a goodbye for one SRV makes that service left, and the other stays live
  - existing GC tests pass unchanged
- [x] 2.3 In the avahi backend, set `src_addr` on the SRV and TXT observations of a `Found` signal to the resolved address (D2). Verify:
  - a store-level test that a system-only TXT with that `src_addr` is assigned to the right twin
  - a wire-provided `src_addr` is not overwritten by the system one
  - `cargo test live_avahi -- --ignored --nocapture` still runs on this machine

## 3. UI: shared name and selection

- [x] 3.1 Add the "Shared name" line to `simple_service` in src/ui/view.rs when `shared_with` is not empty. Verify UI tests for the TeamViewer twin (line lists `DESKTOP-28SEPKJ-2.local`) and an ordinary service (no line).
- [x] 3.2 Make `App` remember the selected service's instance key and host key. In `fix_selection`, when the middle id disappears, prefer the listed service with the same instance and host before falling back to the first entry (D5). Verify UI tests for "Twin appears" and "Twin removed", where the id changes and the selection stays on the same host's service.
- [x] 3.3 Document in the README, next to the merging section, that one instance name announced by several machines is shown once per machine, with a "Shared name" note. Verify the README renders and matches the behaviour in the UI tests.

## 4. Integration check

- [x] 4.1 Run `cargo test` and `cargo clippy --all-targets` and verify both are clean. Then run `mdnsview` on the office LAN and verify:
  - "0" is listed twice, once per PC, each with its own `DyngateID`
  - `DESKTOP-EAOAATI.local` no longer appears as a separate service-less device
  - the two services keep their hosts across several minutes of announcements
