# mdns-discovery Specification

## Purpose

Collects mDNS/DNS-SD records from the local network: a coarse overview from the operating system's mDNS daemon, refined with the application's own multicast socket. Neither source requires elevated privileges.

## Requirements

### Requirement: OS daemon discovery
On platforms with a supported mDNS daemon, the system SHALL enumerate service types, service instances and their resolved host, port, TXT data and addresses through that daemon. On Linux the supported daemon is avahi. On macOS it is mDNSResponder. On Windows it is the DNS Client service, used through the Windows DNS-SD API.

#### Scenario: Avahi reports services
- **WHEN** the application starts on Linux with avahi-daemon running and a printer advertising `_ipp._tcp`
- **THEN** the printer's service instance with host, port and TXT data becomes available to the inventory without waiting for the printer to send unsolicited traffic

#### Scenario: mDNSResponder reports services
- **WHEN** the application starts on macOS and a printer on the LAN advertises `_ipp._tcp`
- **THEN** the printer's service instance with host, port, TXT data and addresses becomes available to the inventory through mDNSResponder, and the system source is named "mDNSResponder"

#### Scenario: Windows DNS reports services
- **WHEN** the application starts on Windows with the DNS Client service running and a printer on the LAN advertises `_ipp._tcp`
- **THEN** the printer's service instance with host, port, TXT data and addresses becomes available to the inventory through the Windows DNS-SD API, and the system source is named "Windows DNS"

#### Scenario: Windows DNS-SD API not available
- **WHEN** the application starts on a Windows version without the DNS-SD API, or with the DNS Client service stopped
- **THEN** the application starts, the system source is reported as unavailable, and discovery continues using only the multicast socket

#### Scenario: Daemon reports a removal
- **WHEN** the OS daemon reports that a previously reported service instance was removed
- **THEN** the system records this as a goodbye for that service instance

### Requirement: Multicast listening without privileges
The system SHALL receive mDNS traffic by binding a UDP socket to port 5353 that shares the port with other mDNS software on the host. It SHALL NOT require root or raw-socket capabilities.

#### Scenario: Running alongside avahi as a normal user
- **WHEN** the application is started by an unprivileged user while avahi-daemon is bound to port 5353
- **THEN** the application receives multicast mDNS packets and avahi continues to function

#### Scenario: Running alongside mDNSResponder as a normal user
- **WHEN** the application is started on macOS by an unprivileged user while mDNSResponder is bound to port 5353
- **THEN** the application receives multicast mDNS packets with their receiving interface, and Bonjour on the Mac continues to function

#### Scenario: Running alongside the Windows DNS Client as a normal user
- **WHEN** the application is started on Windows by a user without administrator rights while the DNS Client service and a web browser are bound to port 5353, and incoming mDNS is allowed for the application
- **THEN** the application receives multicast mDNS packets with their receiving interface, and `.local` name resolution on the PC continues to function

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
For each service instance or host reported by the OS daemon, the system SHALL send its own queries for that instance's SRV and TXT records and the host's A/AAAA records, so that wire-level metadata is captured for it. The same applies to instances discovered on the wire. An instance is refined once its SRV and TXT records have both been received on the wire, a host once an A or AAAA record has; each query asks only for what is missing.

#### Scenario: Instance known only from avahi
- **WHEN** avahi reports an instance that has not yet been seen on the wire
- **THEN** the system queries for its records, and the responses are captured with wire metadata

#### Scenario: SRV received without TXT
- **WHEN** a responder's answer contains the SRV record of an instance but leaves out its TXT record, for example because the answer was too large to carry all additional records
- **THEN** the system sends a query for that instance's TXT record only, and the TXT record received in response is captured with wire metadata

#### Scenario: Host with one address family
- **WHEN** a host's A record has been received on the wire and it has no AAAA record
- **THEN** the host counts as refined and no further address queries are sent for it

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
By default, the system SHALL use every interface that is up, multicast-capable and not loopback, and whose name matches none of `veth*`, `awdl*`, `llw*`, `utun*` and `anpi*`. Configured include and exclude patterns SHALL override this default. On Windows, the interface name is the connection name shown by Windows, such as `WLAN` or `Ethernet 2`.

#### Scenario: Default selection
- **WHEN** the host has interfaces lo, wlan0, docker0 and veth1a2b and no interface configuration is set
- **THEN** the system uses wlan0 and docker0

#### Scenario: Default selection on macOS
- **WHEN** a Mac has interfaces lo0, en0, awdl0, llw0, utun3 and anpi0, all up and multicast-capable except lo0, and no interface configuration is set
- **THEN** the system uses only en0

#### Scenario: Default selection on Windows
- **WHEN** a PC has the connections `WLAN` (connected), `Bluetooth Network Connection` (disconnected) and `Loopback Pseudo-Interface 1`, and no interface configuration is set
- **THEN** the system uses only `WLAN`

#### Scenario: Windows names in patterns
- **WHEN** on Windows the configuration sets `exclude = ["vEthernet (*"]`
- **THEN** Hyper-V and WSL connections such as `vEthernet (WSL)` are not used

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

### Requirement: Local Network permission on macOS
On macOS, when the operating system's Local Network privacy setting blocks the application's multicast traffic, the system SHALL report the wire source as unavailable with a reason that names the Local Network permission, and SHALL continue with the OS daemon if it is available. When started from a terminal, macOS does not apply the setting to the application, so this state does not arise there.

#### Scenario: Local Network access denied
- **WHEN** the application runs on macOS in a context where the Local Network privacy setting applies, access has been denied, and every multicast query it sends fails with "no route to host"
- **THEN** the wire source is reported as unavailable with a reason mentioning the Local Network permission, and discovery continues through mDNSResponder if it is reachable

### Requirement: Windows Firewall detection
On Windows, the system SHALL report that incoming mDNS seems blocked by Windows Firewall when both of these hold for 15 seconds after its first queries were sent: the multicast socket has received no mDNS packet from another host, and the OS daemon has reported at least one service instance. The report SHALL be withdrawn when a packet from another host arrives. The multicast socket SHALL keep running. The system SHALL NOT change firewall settings.

#### Scenario: Firewall drops replies
- **WHEN** the application runs on Windows where a firewall drops all incoming mDNS to it, including replies to its own queries, and Windows DNS has reported services on the LAN
- **THEN** after 15 seconds the system reports that incoming mDNS seems blocked by Windows Firewall, and discovery continues through Windows DNS

#### Scenario: Firewall rule added while running
- **WHEN** the firewall report is shown and the user adds an allow rule for the application, so that a reply from another host arrives
- **THEN** the report is withdrawn and wire records appear without restarting the application

#### Scenario: Default firewall without an allow rule
- **WHEN** the application runs on Windows with the default Windows Firewall, no rule allows incoming UDP 5353 for it, and other hosts answer its queries
- **THEN** no firewall report is made, and the answers are captured as wire records

#### Scenario: Empty network
- **WHEN** the application runs on Windows on a network where no other host announces anything, so Windows DNS reports no service instances
- **THEN** no firewall report is made

### Requirement: Refinement retries
Refinement queries that remain unanswered SHALL be retried a bounded number of times with increasing delays, without waiting for the instance or host to be reported again.

#### Scenario: Unanswered refinement is retried
- **WHEN** a refinement query for an instance's TXT record gets no answer
- **THEN** the system sends the query again after increasing delays, up to its retry limit, even if the instance is not reported again in the meantime

#### Scenario: Retries stop once answered
- **WHEN** the TXT record arrives on the wire after the first retry
- **THEN** no further refinement queries are sent for that instance
