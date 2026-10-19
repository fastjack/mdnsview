# browser-ui Specification

## Purpose

The interactive three-pane terminal interface. It lets casual users quickly find devices and services, and lets advanced users inspect every mDNS record, while the data refreshes in the background.

## Requirements

### Requirement: Three-pane layout
The UI SHALL show three panes side by side. In device mode:
- the left pane lists devices
- the middle pane lists the services of the selected device
- the right pane shows the details of the selected service

#### Scenario: Drilling down
- **WHEN** the user selects a device and then one of its services
- **THEN** the middle pane lists that device's services and the right pane shows that service's details

### Requirement: Device list columns
Each device row SHALL show the device label and the time since the device was last seen, in relative form (for example `now`, `14s`, `3m`). Stale and left devices SHALL be rendered dimmed.

#### Scenario: Relative time display
- **WHEN** a device was last seen 200 seconds ago
- **THEN** its row shows `3m` and the device's label

### Requirement: Background refresh
The UI SHALL update to reflect inventory changes while the user is idle, without requiring a key press. It SHALL update at least once per second.

#### Scenario: New device appears
- **WHEN** a new device is discovered while the user is not pressing any keys
- **THEN** it appears in the device list within one second

### Requirement: Stable selection
Rows SHALL be identified by stable identities, not by their position. When rows are added, removed or reordered, the selection SHALL stay on the same entry. Lists SHALL be sorted alphabetically by label.

#### Scenario: Row inserted above selection
- **WHEN** the user has selected "NAS" and a new device "Apple TV" is discovered
- **THEN** "NAS" remains selected

### Requirement: Selected entries are retained
An entry that is currently selected in the left or middle pane SHALL remain listed after it would otherwise have been removed. It SHALL be marked as gone, together with the time since it was last seen. While a service is selected, the device that contains it is also retained. Once the selection moves off a retained, expired entry, the entry SHALL be removed.

#### Scenario: Selected device expires
- **WHEN** the selected device exceeds the device timeout
- **THEN** it stays in the list, dimmed and marked as gone, and the details pane keeps showing its last known data

#### Scenario: User moves away
- **WHEN** the user moves the selection from an expired, retained device to another device
- **THEN** the expired device is removed from the list

#### Scenario: Retained entry returns
- **WHEN** a retained, expired device sends a record again
- **THEN** it becomes live in place, and the selection is unchanged

### Requirement: Service-type pivot
A pivot key (`tab`) SHALL switch the left pane between devices and service types. In service-type mode:
- the middle pane lists the instances of the selected type, each annotated with its host
- the right pane shows the details of the selected instance

#### Scenario: Browse by service type
- **WHEN** the user presses `tab` and selects "Printer (IPP)"
- **THEN** the middle pane lists every IPP printer instance, each shown with its host name

### Requirement: Filtering
Pressing `/` SHALL open a filter prompt that narrows the left pane. The match SHALL be a case-insensitive substring match. Clearing the filter SHALL list all entries again.

#### Scenario: Clear filter
- **WHEN** the user clears the filter
- **THEN** all entries are listed again

### Requirement: Filter matching in device mode
In device mode, the filter SHALL keep a device when its label or host name matches, or when it has a service whose name, type or readable type name matches.

#### Scenario: Find printers in device mode
- **WHEN** the user types `/printer` in device mode
- **THEN** only devices with a service whose readable type name contains "printer" remain listed, plus devices whose label contains "printer"

### Requirement: Filter matching in service-type mode
In service-type mode, the filter SHALL keep a service type when its type or readable name matches, or when it has an instance whose name or host matches.

#### Scenario: Find by host in service-type mode
- **WHEN** the user types `/brother` in service-type mode
- **THEN** only service types with an instance on a host containing "brother" remain listed

### Requirement: Filter hides selection
If the filter hides the selected entry, the selection SHALL move to the first visible entry, and any retention of the hidden entry SHALL end.

#### Scenario: Selected expired entry filtered out
- **WHEN** a retained, expired device is selected and the user types a filter it does not match
- **THEN** the selection moves to the first matching device and the expired device is removed

### Requirement: Simple details view
By default, the details pane SHALL show:
- instance name and readable service type
- host name, addresses and port
- TXT entries decoded as key/value pairs, sorted by key

#### Scenario: Service details
- **WHEN** an IPP printer service is selected
- **THEN** the details pane shows its name, "Printer (IPP)", host, addresses, port 631 and its decoded TXT keys

### Requirement: Raw records view
A key (`d`) SHALL switch the details pane between the simple view and a raw records view. The raw view SHALL list every record related to the selected service: PTR, SRV, TXT, A/AAAA and any other record types received.

#### Scenario: Toggle to raw view
- **WHEN** the user presses `d` on a selected service
- **THEN** the details pane lists the service's PTR, SRV, TXT and its host's address records

### Requirement: Raw record metadata
For each record, the raw view SHALL show:
- type and data
- remaining and original TTL
- cache-flush bit
- source address and interface
- provenance
- first-seen and last-seen times

#### Scenario: SRV record metadata
- **WHEN** the raw view shows an SRV record received on the wire
- **THEN** it shows priority, weight, port, target, `ttl <remaining>/<original>`, cache-flush state, source address, interface, provenance and first/last-seen times

### Requirement: Raw view value rendering
In the raw view, TXT values that are not valid UTF-8 SHALL be shown in hexadecimal. Expired records SHALL remain visible and be marked expired.

#### Scenario: Binary TXT value
- **WHEN** a TXT entry contains bytes that are not valid UTF-8
- **THEN** the raw view shows that value as hexadecimal

#### Scenario: Expired record
- **WHEN** a record of the selected service has expired
- **THEN** the raw view still lists it, marked expired

### Requirement: Keyboard navigation
The UI SHALL support:
- up/down (and `j`/`k`) to move within a pane
- left/right (and `h`/`l`) to move focus between panes
- `tab` for the pivot, `/` to filter, `d` to toggle the details view
- `q` to quit

The terminal SHALL be restored to its previous state on exit, including exit caused by an error.

#### Scenario: Quit
- **WHEN** the user presses `q`
- **THEN** the application exits and the terminal is restored

### Requirement: Status bar
The UI SHALL show a status bar with:
- the active discovery sources (for example "avahi + wire", "wire only", "system only")
- the number of active interfaces
- the number of listed devices
- a short key hint

#### Scenario: Avahi missing
- **WHEN** the OS daemon is unavailable
- **THEN** the status bar shows "wire only"
