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
- `i` to open the interface picker
- `m` to toggle mouse capture
- `q` to quit

The terminal SHALL be restored to its previous state on exit, including exit caused by an error, with mouse capture turned off.

#### Scenario: Quit
- **WHEN** the user presses `q`
- **THEN** the application exits and the terminal is restored

#### Scenario: Mouse capture released after a crash
- **WHEN** the application exits because of an error or panic while mouse capture is on
- **THEN** the terminal no longer reports mouse events to the shell

#### Scenario: Open interface picker
- **WHEN** the user presses `i`
- **THEN** the interface picker opens

### Requirement: Status bar
The UI SHALL show a status bar with:
- the active discovery sources (for example "avahi + wire", "wire only", "system only")
- the number of active interfaces, and while the runtime selection is restricted, also the number of eligible interfaces (for example `1 of 3 interfaces`)
- the number of listed devices
- whether mouse capture is on or off
- a short key hint

#### Scenario: Avahi missing
- **WHEN** the OS daemon is unavailable
- **THEN** the status bar shows "wire only"

#### Scenario: Mouse state shown
- **WHEN** the user turns mouse capture off with `m`
- **THEN** the status bar shows that the mouse is off

#### Scenario: All interfaces in use
- **WHEN** the selection is all and two interfaces are eligible
- **THEN** the status bar shows `2 interfaces`

#### Scenario: Restricted selection
- **WHEN** three interfaces are eligible and the selection is only enp3s0
- **THEN** the status bar shows `1 of 3 interfaces`

### Requirement: Click to select
While mouse capture is on, a left click on a row in the left or middle pane SHALL focus that pane and select the clicked entry, with the same effect as moving the selection there with the keyboard. A click inside a pane but outside its rows SHALL only focus that pane. A left click in the details pane SHALL focus it.

#### Scenario: Click a device
- **WHEN** the device list shows "Apple TV", "NAS" and "Office Printer" and the user clicks the "NAS" row
- **THEN** the left pane is focused, "NAS" is selected, and the middle pane lists the NAS's services

#### Scenario: Click a service
- **WHEN** the user clicks the second row of the middle pane
- **THEN** the middle pane is focused and that service's details are shown

#### Scenario: Click below the last row
- **WHEN** the user clicks the empty area below the last device
- **THEN** the left pane is focused and the selection is unchanged

#### Scenario: Click in a scrolled list
- **WHEN** the device list is scrolled so the first visible row is the 20th device, and the user clicks the first visible row
- **THEN** the 20th device is selected

### Requirement: Wheel scrolling
While mouse capture is on, the scroll wheel SHALL act on the pane under the pointer, whether or not that pane has focus. In the left and middle panes, each wheel step SHALL move the selection by one entry. In the details pane, each wheel step SHALL scroll the content by one line. Scrolling SHALL NOT change which pane has focus.

#### Scenario: Wheel over the device list
- **WHEN** the details pane has focus and the user scrolls down once over the device list
- **THEN** the next device is selected and the details pane keeps focus

#### Scenario: Wheel over the details pane
- **WHEN** the raw records view is longer than the pane and the user scrolls down over it
- **THEN** the content moves up by one line per wheel step

### Requirement: Mouse toggle
A key (`m`) SHALL turn mouse capture off and on while the application runs. While capture is off, the application SHALL NOT receive mouse events, so the terminal's own text selection works. Mouse capture SHALL be on at startup unless the configuration disables it.

#### Scenario: Turn mouse off to copy text
- **WHEN** mouse capture is on and the user presses `m`
- **THEN** mouse capture is turned off, and clicking and dragging selects terminal text instead of rows

#### Scenario: Turn mouse back on
- **WHEN** mouse capture is off and the user presses `m`
- **THEN** mouse capture is on again and clicks select rows

#### Scenario: Disabled in configuration
- **WHEN** the configuration sets `mouse = false`
- **THEN** the application starts with mouse capture off, and `m` turns it on

### Requirement: Selection follows merges
When the selected device is merged with another device or split into separate devices, the selection SHALL move to the device that contains the previously selected device's host name, instead of jumping to another entry. A retained entry SHALL stay retained when it becomes part of a merged device.

#### Scenario: Selected device is merged
- **WHEN** the user has selected the device `1e8da75b.local` and it is merged into the device "Hyperion"
- **THEN** the merged "Hyperion" device is selected

#### Scenario: Selected device splits
- **WHEN** the user has selected a merged device and its host names stop sharing an address
- **THEN** the device holding the host name the merged device was identified by stays selected

### Requirement: Device host names in details
The details pane SHALL make all host names of a device visible:
- the simple service view SHALL include a "Device" line listing every host name of the service's device when the device has more than one
- when a device is selected and none of its services is, the device details SHALL list every host name together with the device's addresses, number of services and state

The filter SHALL match any of a device's host names.

#### Scenario: Merged device in the service view
- **WHEN** a service of the merged device "Hyperion" is selected
- **THEN** the simple details view shows a "Device" line listing `Hyperion.local` and `1e8da75b.local`

#### Scenario: Single host device
- **WHEN** a service of a device with one host name is selected
- **THEN** no "Device" line is shown

#### Scenario: Filter by secondary host name
- **WHEN** the user types `/1e8da75b` in device mode
- **THEN** the merged device "Hyperion" remains listed

### Requirement: Shared instance name in details
When the selected service's instance name is also announced by other hosts, the simple details view SHALL show a "Shared name" line listing those other host names.

#### Scenario: TeamViewer instance shared by two PCs
- **WHEN** the `0._teamviewer._tcp` service of `DESKTOP-EAOAATI.local` is selected and `DESKTOP-28SEPKJ-2.local` announces the same instance name
- **THEN** the simple details view shows a "Shared name" line listing `DESKTOP-28SEPKJ-2.local`

#### Scenario: Instance name not shared
- **WHEN** a service whose instance name is announced by only one host is selected
- **THEN** no "Shared name" line is shown

### Requirement: Selection follows a shared instance's host
When a second host starts or stops announcing the selected service's instance name, the selection SHALL stay on the service of the same instance name and host.

#### Scenario: Twin appears
- **WHEN** the `0._teamviewer._tcp` service of `DESKTOP-EAOAATI.local` is selected and `DESKTOP-28SEPKJ-2.local` starts announcing the same instance name
- **THEN** the `DESKTOP-EAOAATI.local` service remains selected

#### Scenario: Twin removed
- **WHEN** the `0._teamviewer._tcp` service of `DESKTOP-28SEPKJ-2.local` is selected and the `DESKTOP-EAOAATI.local` service of that instance name is removed
- **THEN** the `DESKTOP-28SEPKJ-2.local` service remains selected

### Requirement: Interface picker
A key (`i`) SHALL open an interface picker over the panes. It SHALL list an "All interfaces" entry first, then every eligible interface by name with its addresses, sorted by name. Each entry SHALL show whether it is checked. Interfaces that are not eligible SHALL NOT be listed, except selected interfaces that are currently absent, which SHALL be listed and marked unavailable.

#### Scenario: Open the picker
- **WHEN** wlan0 and enp3s0 are eligible, veth1a2b is excluded by the default patterns, and the user presses `i`
- **THEN** the picker lists "All interfaces", enp3s0 and wlan0, with "All interfaces" checked, and does not list veth1a2b

#### Scenario: Selected interface is down
- **WHEN** the selection is only wlan0, wlan0 is down, and the user opens the picker
- **THEN** wlan0 is listed, checked and marked unavailable

### Requirement: Choosing interfaces in the picker
In the picker, up/down (and `j`/`k`) SHALL move between entries, and `space` SHALL toggle the entry under the cursor. Checking "All interfaces" SHALL uncheck every single interface. Checking a single interface SHALL uncheck "All interfaces". The `a` key SHALL check "All interfaces" from anywhere in the list.

#### Scenario: Pick one interface
- **WHEN** "All interfaces" is checked and the user moves to enp3s0 and presses `space`
- **THEN** enp3s0 is checked and "All interfaces" is unchecked

#### Scenario: Pick several interfaces
- **WHEN** enp3s0 is checked and the user moves to wlan0 and presses `space`
- **THEN** enp3s0 and wlan0 are both checked

#### Scenario: Back to all
- **WHEN** enp3s0 and wlan0 are checked and the user presses `a`
- **THEN** "All interfaces" is checked and no single interface is checked

### Requirement: Applying the interface choice
`enter` SHALL apply the checked entries as the runtime interface selection and close the picker. `esc` SHALL close the picker without changing the selection. Nothing checked SHALL NOT be applicable: `enter` SHALL keep the picker open and show that at least one entry must be checked. While the picker is open, other keys SHALL NOT act on the panes, and the panes SHALL keep refreshing.

#### Scenario: Apply
- **WHEN** only enp3s0 is checked and the user presses `enter`
- **THEN** the picker closes and discovery uses only enp3s0

#### Scenario: Cancel
- **WHEN** the user checks enp3s0 and presses `esc`
- **THEN** the picker closes and the selection is unchanged

#### Scenario: Nothing checked
- **WHEN** the user unchecks the only checked interface and presses `enter`
- **THEN** the picker stays open with a hint that at least one entry must be checked

#### Scenario: Keys do not leak to the panes
- **WHEN** the picker is open and the user presses `q`
- **THEN** the application keeps running and the picker stays open
