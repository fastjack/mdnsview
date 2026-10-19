# Spec Delta

## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: Keyboard navigation
The UI SHALL support:
- up/down (and `j`/`k`) to move within a pane
- left/right (and `h`/`l`) to move focus between panes
- `tab` for the pivot, `/` to filter, `d` to toggle the details view
- `m` to toggle mouse capture
- `q` to quit

The terminal SHALL be restored to its previous state on exit, including exit caused by an error, with mouse capture turned off.

#### Scenario: Quit
- **WHEN** the user presses `q`
- **THEN** the application exits and the terminal is restored

#### Scenario: Mouse capture released after a crash
- **WHEN** the application exits because of an error or panic while mouse capture is on
- **THEN** the terminal no longer reports mouse events to the shell

### Requirement: Status bar
The UI SHALL show a status bar with:
- the active discovery sources (for example "avahi + wire", "wire only", "system only")
- the number of active interfaces
- the number of listed devices
- whether mouse capture is on or off
- a short key hint

#### Scenario: Avahi missing
- **WHEN** the OS daemon is unavailable
- **THEN** the status bar shows "wire only"

#### Scenario: Mouse state shown
- **WHEN** the user turns mouse capture off with `m`
- **THEN** the status bar shows that the mouse is off
