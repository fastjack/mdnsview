# mdns-discovery Specification

## Purpose

Collects mDNS/DNS-SD records from the local network: a coarse overview from the operating system's mDNS daemon, refined with the application's own multicast socket. Neither source requires elevated privileges.

## Requirements

### Requirement: OS daemon discovery
On platforms with a supported mDNS daemon, the system SHALL enumerate service types, service instances and their resolved host, port, TXT data and addresses through that daemon. On Linux the supported daemon is avahi.

#### Scenario: Avahi reports services
- **WHEN** the application starts on Linux with avahi-daemon running and a printer advertising `_ipp._tcp`
- **THEN** the printer's service instance with host, port and TXT data becomes available to the inventory without waiting for the printer to send unsolicited traffic

#### Scenario: Daemon reports a removal
- **WHEN** the OS daemon reports that a previously reported service instance was removed
- **THEN** the system records this as a goodbye for that service instance

### Requirement: Multicast listening without privileges
The system SHALL receive mDNS traffic by binding a UDP socket to port 5353 that shares the port with other mDNS software on the host. It SHALL NOT require root or raw-socket capabilities.

#### Scenario: Running alongside avahi as a normal user
- **WHEN** the application is started by an unprivileged user while avahi-daemon is bound to port 5353
- **THEN** the application receives multicast mDNS packets and avahi continues to function

### Requirement: IPv4 and IPv6 reception
The system SHALL join the mDNS multicast groups 224.0.0.251 and ff02::fb on every selected interface, unless IPv6 is disabled in configuration.

#### Scenario: IPv6 disabled
- **WHEN** the configuration sets `ipv6 = false`
- **THEN** the system joins only the IPv4 group and sends no IPv6 queries

### Requirement: Record capture with metadata
For every resource record received on the multicast socket, the system SHALL capture the record and its metadata:
- name, type and class
- data
- original TTL
- cache-flush bit
- source address
- receiving interface
- receive time

#### Scenario: Response received
- **WHEN** a response containing SRV, TXT and A records arrives from 192.168.1.40 on interface wlan0
- **THEN** each record is stored with its TTL, cache-flush bit, source 192.168.1.40 and interface wlan0

#### Scenario: Unknown record type
- **WHEN** a response contains a record type the system does not decode
- **THEN** the record is kept with its numeric type and raw data, and is not dropped

### Requirement: Active enumeration
The system SHALL send multicast queries to enumerate service types (`_services._dns-sd._udp.local`) and the instances of every discovered type. It SHALL request multicast responses, so that all answers are observed.

#### Scenario: No OS daemon available
- **WHEN** no OS daemon backend is available
- **THEN** the system still discovers service types and instances using only its own queries

### Requirement: Refinement of daemon results
For each service instance or host reported by the OS daemon, the system SHALL send its own queries for that instance's SRV and TXT records and the host's A/AAAA records, so that wire-level metadata is captured for it.

#### Scenario: Instance known only from avahi
- **WHEN** avahi reports an instance that has not yet been seen on the wire
- **THEN** the system queries for its records, and the responses are captured with wire metadata

### Requirement: TTL-driven re-query
For each known record, the system SHALL send a refresh query when 80%, 85%, 90% and 95% of the record's TTL has elapsed without a new answer, following RFC 6762 section 5.2. A record that has received no answer by the time its TTL expires SHALL be marked expired.

#### Scenario: Live host answers refresh
- **WHEN** a host's A record has a TTL of 120 seconds and the host answers the refresh query sent at 96 seconds
- **THEN** the record's TTL and last-seen time are renewed and no further refresh queries are sent for that cycle

#### Scenario: Host gone silent
- **WHEN** a host answers none of the four refresh queries
- **THEN** its record is marked expired when its TTL runs out

### Requirement: Goodbye handling
A received record with a TTL of zero SHALL be treated as a goodbye for that record.

#### Scenario: Service sends goodbye
- **WHEN** a service sends its SRV record with a TTL of zero
- **THEN** the record is marked as having left, with the time of the goodbye

### Requirement: Interface selection
By default, the system SHALL use every interface that is up, multicast-capable, not loopback, and whose name does not match `veth*`. Configured include and exclude patterns SHALL override this default. The interfaces chosen this way are the eligible interfaces; the runtime selection can narrow them further, but never adds an interface that is not eligible.

#### Scenario: Default selection
- **WHEN** the host has interfaces lo, wlan0, docker0 and veth1a2b and no interface configuration is set
- **THEN** the system uses wlan0 and docker0

#### Scenario: Include list set
- **WHEN** the configuration sets `include = ["enp3s0"]`
- **THEN** only enp3s0 is used, provided it is up and multicast-capable

#### Scenario: Exclude list set
- **WHEN** the configuration sets `exclude = ["docker*"]`
- **THEN** docker0 is not used

#### Scenario: Runtime selection narrows the eligible set
- **WHEN** wlan0 and docker0 are eligible and the runtime selection is only docker0
- **THEN** the system uses docker0 only

### Requirement: Interface change detection
The system SHALL detect interfaces that appear, disappear or change address within 10 seconds, and SHALL join or leave the multicast groups on them accordingly, following the runtime selection.

#### Scenario: Wi-Fi reconnect
- **WHEN** wlan0 goes down and comes back up with a new address
- **THEN** within 10 seconds the system is receiving mDNS traffic on wlan0 again

#### Scenario: Deselected interface reconnects
- **WHEN** the selection is only enp3s0, and wlan0 goes down and comes back up
- **THEN** the system does not join the mDNS groups on wlan0

### Requirement: Degraded operation
The system SHALL continue to run when only one discovery source is available, and SHALL report which sources are active. It SHALL exit with a clear error only when neither source can be started.

#### Scenario: Avahi not running
- **WHEN** the OS daemon cannot be reached
- **THEN** discovery continues using only the multicast socket, and the source status reports "wire only"

#### Scenario: Port cannot be shared
- **WHEN** the multicast socket cannot be bound
- **THEN** discovery continues using only the OS daemon, and the source status reports "system only"

#### Scenario: No source available
- **WHEN** neither the OS daemon nor the multicast socket can be used
- **THEN** the application exits with a non-zero status and an error message naming both failures

### Requirement: Cache-flush scope
A received record with the cache-flush bit SHALL expire, one second later, the other records of the same name and type that were received more than one second earlier (RFC 6762 section 10.2). For SRV and TXT records, it SHALL only expire records from the same host:
- an SRV record expires only SRV records with the same target
- a TXT record expires only TXT records received from the same source address

#### Scenario: Host changes its address
- **WHEN** `nas.local` announces a new A record with the cache-flush bit, and its old A record was received 10 seconds earlier
- **THEN** the old A record expires one second later

#### Scenario: Host changes its port
- **WHEN** `printer.local` announces a new SRV record for its instance with a new port and the cache-flush bit
- **THEN** the old SRV record with the same target expires one second later

#### Scenario: Two hosts announce the same instance name
- **WHEN** `DESKTOP-EAOAATI.local` and `DESKTOP-28SEPKJ-2.local` both announce SRV and TXT records for `0._teamviewer._tcp.local` with the cache-flush bit, a minute apart
- **THEN** neither announcement expires the other host's SRV or TXT record

### Requirement: Runtime interface selection
While the application runs, the user SHALL be able to restrict discovery to a subset of the eligible interfaces, where eligible means the interfaces chosen by "Interface selection". The selection SHALL be either all eligible interfaces or an explicit set of interface names. At startup it SHALL be all. A changed selection SHALL take effect within one second and SHALL NOT be saved.

#### Scenario: Restrict to one interface
- **WHEN** wlan0 and enp3s0 are eligible and the user selects only enp3s0
- **THEN** the system leaves the mDNS groups on wlan0, sends its queries only on enp3s0, and ignores packets received on wlan0

#### Scenario: Back to all
- **WHEN** the selection is restricted to enp3s0 and the user selects all interfaces
- **THEN** the system joins the mDNS groups on every eligible interface again and restarts enumeration of service types and instances

#### Scenario: Ineligible interface never used
- **WHEN** the configuration excludes `docker*` and the user selects all interfaces
- **THEN** docker0 is not used

#### Scenario: Selection not saved
- **WHEN** the user restricts the selection to enp3s0 and restarts the application
- **THEN** all eligible interfaces are used

### Requirement: Explicit selection across interface changes
An explicit selection SHALL keep its interface names when interfaces change. A selected interface that disappears SHALL be used again when it becomes eligible again. An interface that becomes eligible and is not in the explicit selection SHALL NOT be used. When the selection is all, every newly eligible interface SHALL be used.

#### Scenario: Selected interface comes back
- **WHEN** the selection is only wlan0, and wlan0 goes down and comes back up
- **THEN** within 10 seconds the system is receiving mDNS traffic on wlan0 again

#### Scenario: New interface with an explicit selection
- **WHEN** the selection is only wlan0 and enp3s0 comes up
- **THEN** enp3s0 is not used

#### Scenario: New interface with all selected
- **WHEN** the selection is all and enp3s0 comes up
- **THEN** within 10 seconds enp3s0 is used

### Requirement: OS daemon results limited to interfaces in use
Records the OS daemon reports on an interface that is not in use SHALL NOT be added to the inventory. An interface is in use when it is eligible and in the runtime selection. Records the daemon reports without an interface SHALL be kept.

#### Scenario: Avahi reports on an excluded interface
- **WHEN** the configuration excludes `veth*` and avahi reports a service on veth1a2b
- **THEN** that service is not listed

#### Scenario: Avahi reports on a deselected interface
- **WHEN** the selection is only enp3s0 and avahi reports a new printer on wlan0
- **THEN** the printer is not listed

### Requirement: Entries from deselected interfaces age out
When an interface stops being in use, entries already discovered through it SHALL remain until they expire and pass the device timeout, as if their hosts had gone silent. Entries the OS daemon reported only on that interface SHALL be released, not marked as having left. When the interface is in use again, the daemon's current entries for it SHALL be reported again without waiting for the daemon to announce them anew.

#### Scenario: Deselected device fades
- **WHEN** a device was discovered on wlan0 and the user restricts the selection to enp3s0
- **THEN** the device stays listed until its records expire, then becomes stale, and is removed once the device timeout has passed

#### Scenario: Daemon entry released
- **WHEN** a service is known only from avahi on wlan0 and wlan0 is deselected
- **THEN** the service is not marked as left, and it is removed once the device timeout has passed since it was last seen

#### Scenario: Interface selected again
- **WHEN** avahi still holds a service on wlan0 and the user selects wlan0 again
- **THEN** the service is live again within one second

#### Scenario: Seen on two interfaces
- **WHEN** a service is reported by avahi on both wlan0 and enp3s0 and wlan0 is deselected
- **THEN** the service stays live
