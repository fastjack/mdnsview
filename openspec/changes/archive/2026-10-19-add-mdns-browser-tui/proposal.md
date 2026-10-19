# Proposal

## Why

There is no convenient terminal tool for seeing what is advertised over mDNS/DNS-SD on the local network. Most existing tools either show only the coarse view the OS daemon exposes (`avahi-browse`) or require packet capture and root for full detail. `mdnsview` should let casual users quickly find devices and services, and let advanced users drill down into every record mDNS provides. It should run without root and stay portable to macOS and Windows later.

## What Changes

- New Rust terminal application `mdnsview` (ratatui) that discovers mDNS devices and services and refreshes continuously in the background.
- Two discovery sources merged into one view:
  - **OS daemon** through a platform abstraction. On Linux this is avahi, which gives a fast coarse overview. macOS and Windows implementations are future work.
  - **Own UDP multicast socket** on port 5353 (IPv4 and IPv6, shared with the daemon, no root). It listens and sends its own queries to refine what the daemon reported and to collect the details the daemon hides (TTLs, flags, source address, interface, every record type).
- Liveness tracking: known hosts and services are actively re-queried on a schedule derived from each record's TTL. Each device shows its last-seen time and is removed once it has not been seen for a configurable timeout (default 5 minutes).
- Devices are identified by hostname (the SRV target). There is no automatic merging of hosts in v1.
- Three-pane UI: devices → services → details.
  - A pivot key switches the left pane to a service-type view.
  - A `/` filter works in both modes.
  - The details pane switches between a simple view and a raw-records view.
  - Selected entries stay visible after they expire, until the user moves away.
- Configuration file (`config.toml` in the platform config directory) for the device timeout and the network interface selection (include/exclude patterns, IPv6 on/off).
- Out of scope for v1: per-service packet history, macOS/Windows OS-daemon backends, automatic merging of hosts.

## Capabilities

### New Capabilities

- `mdns-discovery`: Collecting mDNS records from the OS daemon and from the app's own multicast socket. Covers active querying and refinement, network interface selection, and operation when one source is unavailable.
- `device-inventory`: Building devices, services and service types from the collected records. Covers hostname grouping, friendly labels, last-seen tracking, liveness states and timeout-based expiry.
- `browser-ui`: The interactive three-pane terminal UI. Covers navigation, the device/service-type pivot, filtering, the simple and raw details views, keeping the selection stable and pinned, and the status bar.
- `app-configuration`: Loading the user configuration file, its defaults, and how invalid configuration is handled.

### Modified Capabilities

None. This is a new project.

## Impact

- New Cargo project in the repository root. Main dependencies: tokio, ratatui/crossterm, zbus (avahi over D-Bus), socket2, a DNS message parser, serde/toml, directories, and an interface enumeration crate.
- Runtime: on Linux, uses avahi-daemon when it is present, and does not require it. Joins the mDNS multicast groups on the selected interfaces. Generates a small amount of multicast query traffic, paced by record TTLs.
- No existing code, APIs or specs are affected.
