# Spec Delta

## ADDED Requirements

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

## MODIFIED Requirements

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
