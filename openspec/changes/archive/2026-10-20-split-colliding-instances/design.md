# Design

## Context

See proposal.md for the motivation and specs/ for the required behaviour. Relevant facts about the current code:

- Observed on the office LAN, in the raw view of the "0" service:
  - one PTR `_teamviewer._tcp.local → 0._teamviewer._tcp.local`
  - two SRV records for `0._teamviewer._tcp.local`, targeting `DESKTOP-28SEPKJ-2.local` (sent from `fe80::c96a:fae5:472d:f9c`) and `DESKTOP-EAOAATI.local` (sent from 192.168.4.135)
  - two TXT records with different `DyngateID`s, from the same two source addresses
  - an NSEC record

  All of them carry the cache-flush bit except the PTR.
- `snapshot::derive` (src/model/snapshot.rs) builds one `Service` per instance key. `best(&own_records, SRV)` picks a single SRV record, preferring live, then most recent. That choice decides `host`, `device_id` and `addresses`, and `best(.., TXT)` picks the TXT. The other host ends up as an address-only `DeviceBuilder` labelled with its host name.
- Records are keyed by (name, data) (`RecordKey`), so both SRV and both TXT records are stored separately. Only the derivation collapses them.
- `Store::apply_cache_flush` (src/model/store.rs) expires every sibling of the same name and type that was last seen on the wire more than 1 s earlier. Both PCs answer the same query within that second, so their records survive each other today. An unsolicited announcement from one PC a minute later would flush the other's records.
- `Store::gc` removes a gone service's `own_keys`, which include the shared PTR.
- The avahi backend (src/backend/avahi.rs) emits SRV and TXT observations without `src_addr`. Its `Found` signal carries the resolved address of the host.
- `App::fix_selection` (src/ui/app.rs) falls back to the first service when the middle id disappears.

## Goals / Non-Goals

**Goals:**
- Keep the derivation a pure function of the stored records, as for device merging: no collision state in the store.
- Leave ids, behaviour and output unchanged for instances with a single SRV target, which is nearly all of them.

**Non-Goals:**
- Splitting by anything other than SRV target, for example two SRV records with the same target and different ports. These stay one service, as today.
- Per-host handling of avahi's `ItemRemove`, which only names the instance.

## Decisions

### D1. One service per SRV target

In `derive`, an instance's records are split by host:
- The SRV records are grouped by `target.key()`.
- No SRV record: one service under "unresolved", as today.
- One target: one service, exactly as today.
- Several targets: one service per target. Each one's own records are:
  - the SRV records with that target
  - the TXT records assigned to that target (D2)
  - the shared PTR records
  - any other records owned by the instance name (NSEC, ...), which are shared like the PTR

`best` is then applied to each target's records to pick the port and TXT. Host records and addresses are computed per service from these records, using the existing helpers.

Liveness, last-seen and `gone` of a split service count only its **life records**: its SRVs and the TXTs assigned to it. Shared records are refreshed by every twin, so counting them would keep a silent twin alive forever. `Service` gains a crate-internal `life_keys` for this. The device-level and per-host liveness and last-seen use it instead of `own_keys`. For an instance that is not split, `life_keys` equals `own_keys`, so nothing changes.

Alternative considered: keep one service and show it on both devices. Rejected. The service would still have one port, one TXT and one state, and the details would mix data from two machines.

### D2. TXT assignment by source address

For an instance with several targets, each TXT record is assigned:
- to the target whose host address records contain the TXT record's `src_addr`
- otherwise it is unassigned

A target with no assigned TXT gets all unassigned ones, so `best` still picks one for it. With a single target, all TXT records belong to it, as today.

To make daemon-only records assignable too, the avahi backend sets `src_addr` on the SRV and TXT observations of a `Found` signal to the resolved address. The store already only fills `src_addr` from the system source when the wire has not provided one, so wire data still wins.

Alternative considered: match the TXT's receive time to the SRV's receive time. Rejected. Both PCs answer within the same few milliseconds.

### D3. Service ids

- One target, or the **first** target: the id stays the instance key. The first target is the one whose SRV record has the earliest `first_seen`, with ties going to the smallest host key.
- Every other target: `"{instance key}\u{0}{host key}"`. The NUL keeps it from ever being equal to a real instance key. `UNRESOLVED_ID` uses the same trick.
- `Service.instance`, `name` and `service_type` are the same for all of them. The service-type list and its sorting therefore show both entries; ties already fall back to the id.

When the first target goes away, the remaining one is renamed back to the plain instance key. D5 keeps the selection on it.

Alternative considered: always qualify ids with the host. Rejected. Every ordinary service would get a new id, and a service whose SRV arrives after its PTR would change id when it leaves "unresolved".

### D4. Cache-flush scope and GC of shared records

`apply_cache_flush` adds one more condition before it expires a sibling:
- **SRV**: only siblings with the same `target.key()`.
- **TXT**: only siblings whose `src_addr` equals the new record's `src_addr`. A sibling without `src_addr` is still flushed, as today.
- All other types: unchanged.

A real port or target change still flushes, because it comes from the same host with the same target, or from the same address. In the worst case, a host that changes its SRV target leaves the old SRV record to expire by TTL (120 s for TeamViewer) instead of after 1 s.

GC: a service's own keys now include records shared with its twins: the PTR, NSEC, and unassigned TXT. When `Store::gc` removes a gone service, it skips keys that are also in the own keys of a service it keeps. This uses the `attributed` pass already in `gc`, applied to the kept services.

### D5. UI

- `Service` gains `shared_with: Vec<DnsName>`, the other targets of the same instance, sorted.
- `simple_service` adds a "Shared name" field listing them when the list is not empty.
- `App` remembers `(instance key, host key)` of the selected service. In `fix_selection`, when the middle id is no longer listed, it first looks for a listed service with the same instance key and host key, and only then falls back to the first entry. Retention keeps working because `retained_ids` uses the current id after the fix.

### D6. Testing

- Model tests on virtual time, built from the observed TeamViewer records:
  - two devices labelled "0", one service each, no address-only device
  - TXT assigned by source address
  - an unassigned TXT shared by both
  - a single target unchanged, with the same id
  - a goodbye for one SRV record, after which the other service stays live
  - one host silent past the timeout: its service is removed, while the PTR and the other service remain
  - cache-flush does not cross hosts for SRV or TXT
  - cache-flush still expires the old SRV record of a port change
- UI tests:
  - "Shared name" shown and hidden
  - selection kept when a twin appears
  - selection kept when the first twin is removed and the id changes
- A manual check on the office LAN: "0" appears twice, once per PC, and "DESKTOP-EAOAATI.local" no longer appears as a separate service-less row.

## Risks / Trade-offs

- [Two distinct devices with the same label "0" are both listed] → This is correct: they are two machines. The device details and the "Shared name" line show which host each one is. They do not merge, because they share no address.
- [An unassigned TXT record from a third, unknown sender is shown on every service of the instance] → This is only a fallback for hosts with no TXT of their own, and the raw view shows each record's source.
- [A host that legitimately moves its service to a new host name briefly shows as two services, until the old SRV expires by TTL] → It lasts at most one SRV TTL. Hosts normally send a goodbye for the old SRV record, which removes it at once.
- [avahi's `ItemRemove` for a shared instance name releases both hosts' daemon-held records] → Wire records keep each service alive on their own TTLs. In system-only mode, both services go together. This is accepted as out of scope.
