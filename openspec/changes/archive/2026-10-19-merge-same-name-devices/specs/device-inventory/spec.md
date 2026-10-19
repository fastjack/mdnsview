# Spec Delta

## MODIFIED Requirements

### Requirement: Devices identified by hostname
The system SHALL represent each distinct host name as one device, unless host names are merged as described in "Merging hosts with the same name". This includes host names that appear as SRV targets and host names that have address records. Host names that only share an address, but not a label, SHALL NOT be merged.

#### Scenario: Multiple services on one host
- **WHEN** instances of `_ipp._tcp`, `_http._tcp` and `_printer._tcp` all have SRV target `brother-a1b2.local`
- **THEN** one device `brother-a1b2.local` is listed with three services

#### Scenario: Two host names share an address
- **WHEN** `nas.local` and `nas-2.local` both resolve to 192.168.1.10 and have different labels
- **THEN** two separate devices are listed

## ADDED Requirements

### Requirement: Merging hosts with the same name
Two host names SHALL be shown as one device when their labels are equal, ignoring case, and they share at least one IP address. A host's addresses for this comparison are those of its live address records, or its last known addresses if none is live. Merging SHALL be transitive. Host names without known addresses SHALL NOT be merged. The "unresolved" device SHALL never be merged.

#### Scenario: One machine under two host names
- **WHEN** `Hyperion.local` has 8 services named "Hyperion", `1e8da75b.local` has 1 service named "Hyperion", and both resolve to 10.10.132.147
- **THEN** one device "Hyperion" is listed with all 9 services

#### Scenario: Same name, different addresses
- **WHEN** `printer-a.local` and `printer-b.local` are both labelled "Office Printer" but have no address in common
- **THEN** two devices labelled "Office Printer" are listed

#### Scenario: Addresses not known yet
- **WHEN** two host names have the same label and no address records have been received for one of them
- **THEN** they are listed as separate devices until a shared address is known

#### Scenario: Shared address goes away
- **WHEN** one host of a merged device announces new addresses, and the old ones are flushed so that no address is shared any more
- **THEN** the hosts are listed as separate devices again

#### Scenario: Stale merged device stays merged
- **WHEN** all address records of a merged device have expired without new addresses being announced
- **THEN** the device stays merged, based on the last known addresses

### Requirement: Merged device contents
A merged device SHALL:
- list the services of all its host names
- show the union of their addresses
- have the label the host names share
- have a last-seen time equal to the latest of its host names' last-seen times
- be in the liveness state computed over the records of all its host names

#### Scenario: Last seen of a merged device
- **WHEN** one host name of a merged device was last seen 10 seconds ago and the other 2 minutes ago
- **THEN** the device's last-seen time is 10 seconds ago

#### Scenario: Live while one host is live
- **WHEN** all records of one host name of a merged device have expired and the other host name still has live records
- **THEN** the device is live

### Requirement: Stale host names leave a merged device
A host name of a merged device that has not been seen for longer than the device timeout SHALL be removed together with its services, while the rest of the device remains. This SHALL NOT happen while the whole device is retained by the UI selection after its timeout.

#### Scenario: One host name goes silent
- **WHEN** one host name of a merged device has not been seen for longer than the device timeout and the other is still answering
- **THEN** the silent host name and its services are removed, and the device remains with the other host name
