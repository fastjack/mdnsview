# Spec Delta

## MODIFIED Requirements

### Requirement: OS daemon discovery
On platforms with a supported mDNS daemon, the system SHALL enumerate service types, service instances and their resolved host, port, TXT data and addresses through that daemon. On Linux the supported daemon is avahi. On macOS it is mDNSResponder.

#### Scenario: Avahi reports services
- **WHEN** the application starts on Linux with avahi-daemon running and a printer advertising `_ipp._tcp`
- **THEN** the printer's service instance with host, port and TXT data becomes available to the inventory without waiting for the printer to send unsolicited traffic

#### Scenario: mDNSResponder reports services
- **WHEN** the application starts on macOS and a printer on the LAN advertises `_ipp._tcp`
- **THEN** the printer's service instance with host, port, TXT data and addresses becomes available to the inventory through mDNSResponder, and the system source is named "mDNSResponder"

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

### Requirement: Interface selection
By default, the system SHALL use every interface that is up, multicast-capable and not loopback, and whose name matches none of `veth*`, `awdl*`, `llw*`, `utun*` and `anpi*`. Configured include and exclude patterns SHALL override this default.

#### Scenario: Default selection
- **WHEN** the host has interfaces lo, wlan0, docker0 and veth1a2b and no interface configuration is set
- **THEN** the system uses wlan0 and docker0

#### Scenario: Default selection on macOS
- **WHEN** a Mac has interfaces lo0, en0, awdl0, llw0, utun3 and anpi0, all up and multicast-capable except lo0, and no interface configuration is set
- **THEN** the system uses only en0

#### Scenario: Include list set
- **WHEN** the configuration sets `include = ["enp3s0"]`
- **THEN** only enp3s0 is used, provided it is up and multicast-capable

#### Scenario: Exclude list set
- **WHEN** the configuration sets `exclude = ["docker*"]`
- **THEN** docker0 is not used

## ADDED Requirements

### Requirement: Local Network permission on macOS
On macOS, when the operating system's Local Network privacy setting blocks the application's multicast traffic, the system SHALL report the wire source as unavailable with a reason that names the Local Network permission, and SHALL continue with the OS daemon if it is available. When started from a terminal, macOS does not apply the setting to the application, so this state does not arise there.

#### Scenario: Local Network access denied
- **WHEN** the application runs on macOS in a context where the Local Network privacy setting applies, access has been denied, and every multicast query it sends fails with "no route to host"
- **THEN** the wire source is reported as unavailable with a reason mentioning the Local Network permission, and discovery continues through mDNSResponder if it is reachable
