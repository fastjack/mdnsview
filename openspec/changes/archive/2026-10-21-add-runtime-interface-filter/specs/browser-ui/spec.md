# Spec Delta

## ADDED Requirements

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

## MODIFIED Requirements

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
