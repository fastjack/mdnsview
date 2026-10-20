# Spec Delta

## MODIFIED Requirements

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

## ADDED Requirements

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
