# Spec Delta

## Purpose

Turns collected mDNS records into the devices, services and service types the user browses. It tracks when each was last seen and expires entries that have disappeared from the network.

## ADDED Requirements

### Requirement: Devices identified by hostname
The system SHALL represent each distinct host name as one device. This includes host names that appear as SRV targets and host names that have address records. Different host names SHALL NOT be merged automatically, even when they share an address.

#### Scenario: Multiple services on one host
- **WHEN** instances of `_ipp._tcp`, `_http._tcp` and `_printer._tcp` all have SRV target `brother-a1b2.local`
- **THEN** one device `brother-a1b2.local` is listed with three services

#### Scenario: Two host names share an address
- **WHEN** `nas.local` and `nas-2.local` both resolve to 192.168.1.10
- **THEN** two separate devices are listed

### Requirement: Host-only and unresolved entries
A host name that has address records but no services SHALL be listed as a device with no services. A service instance whose host name is not yet known SHALL be listed under a dedicated "unresolved" device.

#### Scenario: Address record without services
- **WHEN** only an A record for `router.local` has been received
- **THEN** a device `router.local` is listed with no services

#### Scenario: Instance without SRV
- **WHEN** a PTR names an instance whose SRV record has not been received
- **THEN** the instance appears under the "unresolved" device, and moves to its host's device once the SRV record arrives

### Requirement: Friendly device label
Each device SHALL have a display label, chosen in this order:
1. the instance name used by most of the host's services
2. the `model` value from the host's `_device-info._tcp` TXT record
3. the host name

#### Scenario: Instance name used as label
- **WHEN** a device has two services named "Office Printer" and one named "Brother Web"
- **THEN** the device label is "Office Printer"

#### Scenario: Instance name preferred over model
- **WHEN** a device has a service named "Living Room" and device-info with `model=AppleTV11,1`
- **THEN** the device label is "Living Room"

#### Scenario: Model used without services
- **WHEN** a device has no services yet and device-info with `model=AppleTV11,1`
- **THEN** the device label is "AppleTV11,1"

#### Scenario: Fallback to host name
- **WHEN** a device has no services and no device-info
- **THEN** the device label is its host name

### Requirement: Service types with readable names
The system SHALL group service instances by service type. A built-in table SHALL map common types to human-readable names, for example `_ipp._tcp` → "Printer (IPP)". Unknown types SHALL be shown by their raw type string.

#### Scenario: Known type
- **WHEN** instances of `_airplay._tcp` exist
- **THEN** a service type with readable name "AirPlay" is listed

#### Scenario: Unknown type
- **WHEN** instances of `_foo-bar._tcp` exist
- **THEN** a service type labelled `_foo-bar._tcp` is listed

### Requirement: Last-seen tracking
Each device and service SHALL have a last-seen time. It SHALL be set to the time the most recent record about that entry was received from any source. Records about a device include the host's address records and the records of the device's services.

#### Scenario: Refresh answer updates last seen
- **WHEN** a device answers an address refresh query
- **THEN** its last-seen time becomes the time of that answer

#### Scenario: System-only operation
- **WHEN** the multicast socket is unavailable and the OS daemon still reports a service instance it has not removed
- **THEN** the service and its device are treated as seen at every inventory update, so they are not removed while the daemon holds them

### Requirement: Source provenance
Each record SHALL record which sources reported it: OS daemon, multicast socket, or both.

#### Scenario: Reported by both sources
- **WHEN** a TXT record is reported by avahi and also received on the wire
- **THEN** its provenance is "both"

### Requirement: Liveness states
Each device and service SHALL be in exactly one of these states:
- **live**: at least one non-expired record
- **stale**: all records expired, and no goodbye was received
- **left**: a goodbye was received, or the OS daemon reported the entry removed

Receiving any new record SHALL return the entry to live.

#### Scenario: Records expire
- **WHEN** all of a device's records pass their TTL without renewal
- **THEN** the device becomes stale

#### Scenario: Device returns
- **WHEN** a stale or left device sends any record
- **THEN** the device becomes live again and keeps its identity

### Requirement: Timeout-based removal
A device or service SHALL be removed when the time since it was last seen exceeds the configured device timeout (default 5 minutes). The one exception is an entry retained by the UI selection. A service type SHALL be removed when it has no remaining instances.

#### Scenario: Silent device removed
- **WHEN** a device has not been seen for 5 minutes and 1 second with the default timeout, and it is not selected
- **THEN** the device is no longer listed

#### Scenario: Custom timeout
- **WHEN** the configured device timeout is 10 minutes and a device was last seen 6 minutes ago
- **THEN** the device is still listed
