# Spec Delta

## Purpose

Collects mDNS/DNS-SD records from the local network: a coarse overview from the operating system's mDNS daemon, refined with the application's own multicast socket. Neither source requires elevated privileges.

## ADDED Requirements

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
By default, the system SHALL use every interface that is up, multicast-capable, not loopback, and whose name does not match `veth*`. Configured include and exclude patterns SHALL override this default.

#### Scenario: Default selection
- **WHEN** the host has interfaces lo, wlan0, docker0 and veth1a2b and no interface configuration is set
- **THEN** the system uses wlan0 and docker0

#### Scenario: Include list set
- **WHEN** the configuration sets `include = ["enp3s0"]`
- **THEN** only enp3s0 is used, provided it is up and multicast-capable

#### Scenario: Exclude list set
- **WHEN** the configuration sets `exclude = ["docker*"]`
- **THEN** docker0 is not used

### Requirement: Interface change detection
The system SHALL detect interfaces that appear, disappear or change address within 10 seconds, and SHALL join or leave the multicast groups on them accordingly.

#### Scenario: Wi-Fi reconnect
- **WHEN** wlan0 goes down and comes back up with a new address
- **THEN** within 10 seconds the system is receiving mDNS traffic on wlan0 again

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
