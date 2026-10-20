# Spec Delta

## ADDED Requirements

### Requirement: Instance names announced by several hosts
When SRV records for one service instance name point at more than one host name, the system SHALL list one service per host name. Each service belongs to the device of its own host and shows that host's port, addresses and records. The PTR records of the instance are shared by all of these services.

#### Scenario: Two PCs announce the same TeamViewer instance
- **WHEN** `0._teamviewer._tcp.local` has an SRV record with target `DESKTOP-EAOAATI.local` and another with target `DESKTOP-28SEPKJ-2.local`, and both hosts have address records
- **THEN** two devices are listed, each labelled "0" with one `_teamviewer._tcp` service, and no service-less device for either host name

#### Scenario: Service stays on its host
- **WHEN** the two SRV records of a shared instance name are refreshed at different times
- **THEN** each service keeps its own host, and neither moves to the other device

#### Scenario: Single announcer unchanged
- **WHEN** an instance name has SRV records with only one target
- **THEN** exactly one service is listed for it, as before

### Requirement: TXT data of a shared instance name
For an instance name announced by several hosts, a TXT record SHALL belong to the host whose addresses include the address the record was received from. A TXT record that cannot be assigned this way SHALL be used for each host of the instance that has no TXT record of its own.

#### Scenario: TXT assigned by source address
- **WHEN** a TXT record with `DyngateID=925584742` was received from 192.168.4.135, which is an address of `DESKTOP-EAOAATI.local`, and one with `DyngateID=1800894754` from `fe80::c96a:fae5:472d:f9c`, an address of `DESKTOP-28SEPKJ-2.local`
- **THEN** the `DESKTOP-EAOAATI.local` service shows `DyngateID=925584742` and the `DESKTOP-28SEPKJ-2.local` service shows `DyngateID=1800894754`

#### Scenario: TXT without a known source
- **WHEN** a shared instance name has one TXT record whose source address matches neither host
- **THEN** both services show that TXT record

### Requirement: Per-host lifetime of a shared instance name
Each service of an instance name announced by several hosts SHALL have its own liveness state, last-seen time and timeout removal. These SHALL be computed from its own SRV records and the TXT records assigned to it. Records shared with the other hosts' services, such as the PTR, SHALL NOT count, so that one host's answers do not keep another host's service alive. Goodbyes from one host SHALL NOT affect the other hosts' services.

#### Scenario: One PC leaves
- **WHEN** `DESKTOP-28SEPKJ-2.local` sends goodbyes for its SRV and TXT records of `0._teamviewer._tcp.local`
- **THEN** its service becomes left, and the `DESKTOP-EAOAATI.local` service stays live

#### Scenario: One PC goes silent
- **WHEN** one host's SRV record of a shared instance name has not been seen for longer than the device timeout, and the other host keeps answering
- **THEN** the silent host's service is removed and the other host's service remains, with its PTR record still present
