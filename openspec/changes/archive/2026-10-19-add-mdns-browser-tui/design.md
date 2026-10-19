# Design

## Context

This is a greenfield project with no existing code; see proposal.md for the motivation and specs/ for the required behaviour. These constraints shape the design:

- **Rust + ratatui, single binary.** Linux is the only target platform now. macOS and Windows must be possible later without restructuring.
- **No root.** All network access goes through an ordinary UDP socket that shares port 5353 with the OS daemon.
- **Two sources with different fidelity.** The OS daemon (avahi) is fast and coarse, with no TTLs, source addresses or flags. The wire socket gives full detail, but only for what it receives or queries.
- **mDNS hosts are mostly silent**, so liveness has to be driven by the application's own queries (TTL-driven refresh per RFC 6762 §5.2).

## Goals / Non-Goals

**Goals:**
- One record store fed by independent sources, so a new platform backend only adds another source.
- Deterministic and testable lifecycle logic (TTL, refresh, expiry, retention) with no real network or real clock.
- A UI that never blocks on the network and never jumps under the user's cursor.

**Non-Goals:**
- Acting as an mDNS responder, or announcing anything.
- Packet history and pcap-level capture (v2).
- Unicast DNS-SD (wide-area Bonjour). Only `.local` is supported.
- Netlink-based interface monitoring. Polling is enough for v1 and is portable.

## Decisions

### D1. Architecture: sources → store task → snapshot → UI

```
 +-----------------+     +-----------------+
 | SystemBackend   |     | WireBackend     |
 | (Avahi | Null)  |     | sockets, codec, |
 +--------+--------+     | query scheduler |
          |              +----+-------+----+
          | Observation       |       ^ QueryRequest
          v                   v       |
       +-------------------------------+     Retain(ids)
       | Store task (sole owner)       | <------------------+
       | records, TTL, cache-flush,    |                    |
       | liveness, GC                  |                    |
       +---------------+---------------+                    |
                       | watch::Sender<Arc<Snapshot>>       |
                       v                                    |
       +-------------------------------+                    |
       | UI loop: crossterm EventStream| -------------------+
       | + 1s tick + snapshot changes  |
       +-------------------------------+
```

- Everything runs as tokio tasks. Backends send `Observation` messages over an `mpsc` channel.
- The store is owned by a single task, so it needs no locks. It publishes an immutable `Arc<Snapshot>` through `tokio::sync::watch`, coalesced to at most 4 publishes per second.
- The UI only reads snapshots and sends the IDs it wants retained (D7).
- Alternative considered: a shared store behind `Arc<Mutex<_>>`. Rejected because rendering would contend with packet ingestion, and lifecycle tests would get harder.

### D2. `SystemBackend` trait with an avahi implementation over zbus

- The trait is an async source. It reports type/instance/resolve results and removals as `Observation`s tagged `Source::System`, and has a `name()` for the status bar.
- `AvahiBackend` uses `zbus`, a pure-Rust D-Bus client that works with tokio, so there is no libdbus C dependency.
- The avahi call flow is:
  1. `ServiceTypeBrowserNew`
  2. `ServiceBrowserNew` for each type
  3. `ServiceResolverNew` for each instance

  Resolver results carry host, address, port, TXT and interface.
- `NullBackend` is used when avahi is unreachable, on other OSes until their backends exist, and in tests.
- Records reported by avahi have no real TTL. They count as present until avahi sends `ItemRemove`. In system-only mode they are treated as seen at every update (device-inventory spec, scenario "System-only operation").
- Alternatives considered:
  - Shelling out to `avahi-browse -p`: fragile to parse, and adds a process dependency.
  - Linking `libavahi-client`: adds a C dependency and hurts portability.

### D3. Wire sockets: socket2 for setup, nix `recvmsg` for interface info

- Two sockets: `0.0.0.0:5353` and `[::]:5353` (the latter with `IPV6_V6ONLY`). Both use `SO_REUSEADDR` and `SO_REUSEPORT`, are non-blocking, and are wrapped in `tokio::io::unix::AsyncFd`.
- Group membership is per interface: `IP_ADD_MEMBERSHIP` with the interface address, and `IPV6_JOIN_GROUP` with the interface index.
- `IP_PKTINFO` / `IPV6_RECVPKTINFO` are enabled, and `nix::sys::socket::recvmsg` reads the receiving interface index. This fills the record's interface metadata, which a plain `recv_from` cannot provide.
- Sending sets `IP_MULTICAST_IF` / `IPV6_MULTICAST_IF` per interface, then sends to the group. Queries are sent from port 5353 with the unicast-response bit cleared (QM), so answers are multicast and also observed by us.
- The platform-specific parts (pktinfo, reuseport) sit behind a small `wire::sys` module, so Windows can supply its own implementation later.

### D4. DNS codec: `simple-dns`

- `simple-dns` was designed for mDNS:
  - it exposes the cache-flush bit on records and the unicast-response bit on questions
  - it parses without allocating
  - it keeps unknown record types as raw data
- A thin `wire::codec` module converts its types into our own `Record` type, so the rest of the code doesn't depend on the parser.
- Alternative considered: `hickory-proto`. It is heavier, and its mDNS bit handling is less direct. The codec module keeps a later switch cheap.

### D5. Query scheduler

The scheduler is a single timer wheel (a `BTreeMap<Instant, Vec<Query>>`) inside the wire backend. It handles three kinds of queries:

- **Enumeration.** Queries for `_services._dns-sd._udp.local` PTR, and a PTR query for each known type. The interval starts at 1 s and doubles each time up to 60 min (RFC 6762 §5.2 continuous querying). Known answers with more than 50% of their TTL left are included, so responders stay quiet.
- **Refinement.** A `QueryRequest` from the store, sent when it learns of an instance or host without wire data. This triggers immediate SRV/TXT for the instance and A/AAAA for its host. Repeated requests for the same name are ignored for 1 s.
- **Refresh.** For each PTR/SRV/TXT/A/AAAA record received on the wire, queries are scheduled at 80/85/90/95% of its TTL, each with 0–2% random jitter. Receiving a fresh copy of the record cancels the pending refreshes.
  - **Liveness window.** For SRV and A/AAAA records, the window is capped at half the device timeout. Some Apple devices and sleep proxies use a 4500 s TTL for addresses; refreshing only at 80% of that would let a healthy, silent device pass the 5-minute timeout and be removed.

Questions that fall due within the same 20 ms are combined into one packet per interface, to keep traffic low.

### D6. Store data model

- A record is keyed by `(name lowercased, rtype, rdata bytes)`, because PTR/A/AAAA can have several values per name.
- Per record, the store keeps:
  - the decoded data
  - the original TTL and its expiry `Instant`
  - the cache-flush bit
  - the source set
  - the last source address and interface
  - first/last seen, as both `Instant` and `SystemTime` (the latter for display)
  - an optional goodbye time
- Cache-flush (RFC 6762 §10.2): a record received with the bit set marks other records with the same name and type as expiring 1 s later, if they were received more than 1 s ago.
- A TTL of 0 sets the goodbye time.
- `Snapshot` derivation does a full rebuild on each publish. That is cheap at LAN scale (hundreds of records).
  - devices: SRV targets ∪ A/AAAA owner names
  - services: instance → SRV/TXT, plus an "unresolved" bucket
  - types: type → instances
  - labels and readable names (from a static `service_names` table)
  - liveness state, computed per device-inventory spec
- GC runs once per second. It removes devices/services whose last-seen time is older than `device_timeout` and whose ID is not in the retain set, along with their records.

### D7. Selection retention

- The UI keys selection by ID: device = host name, service = instance full name, type = type string.
- After each selection change or filter change, the UI sends `Retain(Vec<Id>)` to the store. The list contains the selected device or type, the selected service, and the service's device.
- The store skips GC for those IDs, and the snapshot marks them `retained` so they render as "gone Xm (pinned)".
- The alternative was for the UI to keep its own copy of expired entries. Rejected, because the details pane needs the full record set, and the store already has it.

### D8. UI loop and rendering

- The loop uses `tokio::select!` over three inputs:
  - the crossterm `EventStream`
  - `watch::Receiver::changed()`
  - a 1 s interval, for relative times and TTL countdown
- It redraws only when one of them fires.
- `App` state (focus, mode, filter, selected IDs, detail mode) is kept separate from rendering, so key handling can be unit-tested.
- Rendering uses ratatui `List`/`Paragraph` widgets with stable-ID-to-index resolution on every draw.
- A panic hook and a drop guard restore the terminal.

### D9. Configuration and interfaces

- Configuration:
  - `serde` + `toml` with `deny_unknown_fields`
  - `humantime-serde` for durations
  - `directories::ProjectDirs` for the platform path
  - `globset` for interface patterns
- `clap` handles `--config`.
- Interface enumeration uses `nix::ifaddrs::getifaddrs` (flags + addresses) behind `wire::sys`, and is polled every 5 s. The scheduler joins and leaves groups based on differences from the previous poll.

### D10. Crate layout

There is one binary crate with these modules:

| Module | Contents |
|--------|----------|
| `config` | configuration loading |
| `model` | `record`, `store`, `snapshot`, `service_names` |
| `backend` | `SystemBackend` trait, `avahi`, `null` |
| `wire` | `sys`, `socket`, `codec`, `scheduler`, `interfaces` |
| `ui` | `app`, `view`, `keys` |
| `main` | entry point |

A Cargo workspace isn't needed yet.

### D11. Testing strategy

- The codec is tested against captured mDNS packets stored as byte fixtures under `tests/fixtures/`.
- The store and scheduler are tested with `#[tokio::test(start_paused = true)]`, so TTL, refresh, GC and retention run on virtual time.
- The UI is tested by feeding key events into `App` and rendering through ratatui's `TestBackend` with buffer assertions.
- The avahi backend is verified manually against `avahi-publish` on the development machine. CI doesn't need D-Bus.

## Risks / Trade-offs

- [Avahi D-Bus race: browser signals can be emitted before a client subscribes to the new object path] → Install signal match rules for the avahi browser and resolver interfaces before calling `*New`, then filter incoming signals by object path.
- [Port sharing differs by OS; Windows Dnscache may refuse it] → Socket setup is isolated in `wire::sys`, and a bind failure already degrades to "system only".
- [Our own query traffic] → Queries follow TTL pacing, exponential backoff for enumeration, known-answer suppression and per-name deduplication. Traffic is roughly one small packet per host every ~100 s.
- [Host-name grouping splits one physical device (IPv4/IPv6 names, sleep proxies)] → An accepted v1 limitation. The raw view shows addresses, so users can see the relationship.
- [Large networks (thousands of records) make the full snapshot rebuild slow] → Publishes are coalesced to at most 4 per second. If profiling shows a problem, switch to incremental derivation.
- [Loopback of our own queries] → Received queries are parsed but produce no records. Our own host's responses are legitimate and are shown.
