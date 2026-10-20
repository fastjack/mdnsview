# Proposal

## Why

On the office LAN, the left column shows a device "0" with one `_teamviewer._tcp` service, plus a second, service-less device "DESKTOP-EAOAATI.local", which is the same PC. This is not a failure of the same-name merge. TeamViewer on two different Windows PCs (`DESKTOP-EAOAATI.local` at 192.168.4.135, and `DESKTOP-28SEPKJ-2.local` at `fe80::c96a:fae5:472d:f9c`) announces the same instance name `0._teamviewer._tcp.local`, without the mDNS conflict resolution that would rename one of them. `mdnsview` identifies a service by its instance name. The two machines' SRV and TXT records therefore end up in one service, and that service:

- shows the SRV target and TXT of whichever record is "best" at the moment, so its host and `DyngateID` flip between the two PCs
- leaves the other PC as an address-only device labelled with its host name

Both PCs should appear as their own device, each with its own TeamViewer service.

## What Changes

- When one instance name has SRV records pointing at more than one host, each of those hosts gets its own service for that instance. Each of those services shows the port, TXT data, addresses and records of its own host.
- TXT records are assigned to a host by the address they were sent from. A TXT record that cannot be assigned is shown on every host of that instance that has no TXT record of its own.
- A service that has no colliding twin keeps its current id. Selection and retention of ordinary services are unchanged.
- Cache-flush on an SRV or TXT record no longer expires the matching record of a *different* host. Without this, two colliding announcers would keep flushing each other's records.
- Liveness, last-seen, timeout removal and goodbyes work per host for colliding instances. One PC leaving does not remove the other's service.
- The service details show a "Shared name" note when an instance name is announced by more than one host.
- Out of scope:
  - renaming or deduplicating colliding instances in the display (both keep the name "0")
  - detecting conflicts on records other than SRV/TXT
  - per-host handling of an OS-daemon "removed" event, which still applies to the whole instance name

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `device-inventory`:
  - new requirement: an instance name announced by several hosts is split into one service per host, with TXT records assigned by source address
  - new requirement: liveness and removal of colliding services are tracked per host
- `mdns-discovery`: cache-flush of SRV/TXT records is limited to records from the same host
- `browser-ui`:
  - service details mark an instance name shared by several hosts
  - the selection stays on the same host's service when a twin appears or goes away

## Impact

- Code: `src/model/snapshot.rs`: instance derivation groups SRV records by target and builds one `Service` per target; TXT is assigned by source address.
- Code: `src/model/store.rs`: `apply_cache_flush` skips SRV/TXT siblings from another host. GC keeps instance records that are still used by another service.
- Code: `src/backend/avahi.rs`: SRV/TXT observations from a resolve carry the resolved address as `src_addr`, so daemon-only records can be assigned too.
- Code: `src/ui/view.rs`: "Shared name" line in the simple service view.
- Code: `src/ui/app.rs`: middle-pane selection falls back to the same instance and host when the service id changes.
- Tests: model tests built from the observed TeamViewer records. Optionally a wire fixture.
- No new dependencies and no configuration changes.
