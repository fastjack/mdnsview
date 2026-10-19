# Tasks

## 1. Derivation: hosts, then merged devices

- [x] 1.1 Split `snapshot::derive` into a per-host stage (`HostView`: host records, service ids, own label, comparison addresses, own last-seen and liveness) and a device stage, with no behaviour change yet; verify all existing model and UI tests pass unchanged
- [x] 1.2 Group hosts with a union-find on equal lowercased label + intersecting comparison addresses (live addresses, falling back to last known; never the "unresolved" bucket or hosts without addresses); verify model tests for:
  - one machine under two host names (the Hyperion case: 9 services, merged addresses)
  - same name with different addresses
  - addresses not known yet
  - transitive merge of three hosts
  - "unresolved" never merged
- [x] 1.3 Build merged `Device`s:
  - id = primary host (most services, ties → smallest key)
  - `hosts` (primary first) and internal `members`
  - shared label
  - union of addresses
  - last-seen and liveness over all members
  - `Service.device_id` = device id
  - `retained` when the retain set holds the id or any member host key

  Verify model tests for "last seen of a merged device", "live while one host is live", stable id across a merge, and retention via a member host key.
- [x] 1.4 Verify splitting: a cache-flushed address change splits the device with the primary keeping its id, and a fully expired merged device stays merged; verify the "shared address goes away" and "stale merged device stays merged" model tests
- [x] 1.5 Update the existing `shared_address_does_not_merge_hosts` test to the new rule (different labels stay separate); verify it passes

## 2. Garbage collection per member

- [x] 2.1 Make `Store::gc` work per member (design D3): a gone, non-retained device is removed whole; a gone, retained device is kept whole; otherwise a member that is gone on its own is removed with its services, unless one of them is retained. Verify model tests for:
  - one host name going silent while the other answers
  - a retained, gone merged device kept whole until released
  - the existing single-host GC tests unchanged

## 3. UI

- [x] 3.1 Make `App::fix_selection` follow host membership in device mode, keeping the middle selection when it is still listed; verify UI tests for the "selected device is merged" and "selected device splits" scenarios
- [x] 3.2 Make the device filter match any of `Device.hosts`; verify a UI test that `/1e8da75b` keeps the merged "Hyperion" device
- [x] 3.3 Show every host name: a `Hosts` field in the device details, and a `Device` line in the simple service view when the device has more than one host name. Verify `TestBackend` tests that a merged device's service shows both host names and that a single-host device's service shows no `Device` line

## 4. Documentation and integration

- [x] 4.1 Describe merging in the README usage section (same name + shared address, always on); verify the text matches the spec's rule
- [x] 4.2 Run `cargo clippy --all-targets -- -D warnings` and `cargo test`, then check on the office LAN in tmux that "Hyperion" is listed once with all its services and both host names in its details; record the results in the change notes

## Workflow follow-up

- Archive the change after review with `/opsx:archive`.
