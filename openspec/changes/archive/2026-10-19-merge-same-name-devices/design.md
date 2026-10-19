# Design

## Context

See proposal.md for motivation and specs/ for the required behaviour. Relevant facts about the current code:

- `snapshot::derive` (src/model/snapshot.rs) builds one `DeviceBuilder` per host key (SRV targets ∪ address-record owners, plus the "unresolved" bucket). It then turns each into a `Device` with `id = host key`, `host`, `host_keys` (the host's own records), its service ids, and a label from `device_label`.
- `Service.device_id` points at the device id. It is used by the UI for pivoting and retention.
- `Store::gc` (src/model/store.rs) works per device:
  - a gone, non-retained device loses its `host_keys` and all its services' records
  - a gone service loses its own records
- `App` (src/ui/app.rs) keys the left selection by device id. `fix_selection` falls back to the first entry when the id disappears. The filter matches `Device.host`.
- The office LAN case: `Hyperion.local` (8 services) and `1e8da75b-…5c59.local` (1 service). Both are labelled "Hyperion" and share `10.10.132.147` and `fe80::c05:d0d4:84b1:b726`.

## Goals / Non-Goals

**Goals:**
- Merge without new state in the store: merging is a pure function of the records, recomputed for every snapshot like the rest of the derivation.
- Keep device ids stable through merges, so selection and retention keep working.

**Non-Goals:**
- Merging by address alone, or by any rule other than "same label + shared address".
- Merging the "unresolved" bucket.

## Decisions

### D1. Two-stage derivation: hosts, then devices

The existing per-host code becomes a `HostView`:
- host name, host record keys and records, service ids
- the host's own label, from `device_label` as today
- its comparison addresses
- its own last-seen time and liveness

Grouping:
- Hosts are grouped with a small union-find. Two hosts are joined when their labels compare equal under `to_lowercase()` and their comparison-address sets intersect.
- To find candidates, hosts are bucketed by lowercased label, then pairs are checked within each bucket. Buckets are tiny, so the pairwise check is cheap.
- The "unresolved" bucket and hosts without addresses are never joined.

Comparison addresses are those of the host's live A/AAAA records, or all its known address records when none is live (spec: "Stale merged device stays merged"). Cache-flush already expires superseded addresses within 1 s, so a real address change shows up quickly.

Alternative considered: merge inside the store, by rewriting device membership on ingest. Rejected; derived state in the store would have to be undone on every split.

### D2. Device identity = primary host

A merged device's id is the key of its **primary host**: the member with the most services, with ties going to the alphabetically smallest key.

- When `1e8da75b…` (1 service) merges with `Hyperion.local` (8), the device id stays `hyperion.local`, the id the larger part already had.
- On a split, the primary keeps its id.

`Device` gains:
- `hosts: Vec<DnsName>`: primary first, then the rest sorted
- `members: Vec<Member>`, crate-internal, for GC: per host, its record keys, its service ids, and whether that host on its own is past the timeout

`Device.host` remains the primary host. `Service.device_id` is set to the device id after grouping.

Label: the shared label, using the primary host's spelling.

Addresses: the sorted union. Last-seen and liveness are computed over the records of all members and their services, using the existing `liveness` and `last_seen` helpers.

### D3. GC per member

For each device, `Store::gc`:

1. If the device is gone and retained, keeps everything (as today).
2. If the device is gone and not retained, removes all members' host records and service records (as today, generalised to members).
3. Otherwise, removes each member that is gone on its own, unless it is held. Its host records and its services' records go. Then it applies the existing per-service rule.
   - A member is **held** when its own host key or one of its services is in the retain set. This keeps a selected merged device's id stable even when its primary host goes silent while another host still answers.
   - A held member also keeps its services. Without them its label would fall back to its host name and the device would split.

A device with a single member behaves exactly as before.

`Device.retained` is true when the retain set contains the device id **or** any member host key. This covers the moment between a merge and the UI's next retain update, when the UI still retains the old id.

### D4. UI: selection and filtering follow hosts

`App::fix_selection` changes in device mode. When `self.left` is no longer a device id, it looks for a visible device whose `hosts` contain that key. If it finds one, it selects that device and keeps the middle selection if that service is still listed. Only otherwise does it fall back to the first entry. Since device ids are host keys, this covers both directions:
- **merge:** the old id is now a non-primary member
- **split:** the primary keeps its id

The device filter matches any entry of `hosts`, not only `host`.

Showing the host names:
- `device_details` in view.rs shows a `Hosts` field listing every host name, one per line, instead of the single `Host` field.
- Because the middle pane always selects a service, a merged device is normally seen through its service details. So the simple service view adds a `Device` line listing all host names, shown only when the service's device has more than one.

### D5. Testing

- Model tests on virtual time:
  - the Hyperion case (9 services, merged addresses)
  - same label without a shared address
  - address unknown for one host
  - a split after a cache-flushed address change
  - a stale merged device staying merged
  - last-seen = max
  - live while one member is live
  - removal of one silent member while the other stays
  - transitive merge of three hosts
  - unresolved never merged
  - a retained gone merged device kept whole
- UI tests:
  - selection follows a merge (select `1e8da75b.local`, then the merge happens)
  - selection survives a split
  - the filter matches a secondary host name
  - the details list both hosts
- A manual check on the office LAN confirms the "Hyperion" row appears once with 9 services.

## Risks / Trade-offs

- [Two different machines with the same name behind one shared address (e.g. a NAT gateway or sleep proxy answering for both)] → Both conditions must hold. Sleep proxies answer with the sleeping host's own records, and distinct machines rarely share both a label and an address. If they do, the raw view still shows each record's source host.
- [Label changes, e.g. a new service changes a host's majority name, can merge or split devices at runtime] → The selection follows hosts (D4), so the UI does not jump.
- [Grouping cost] → O(n) bucketing plus pairwise checks within equal-label buckets; negligible at LAN scale.
