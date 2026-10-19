# Proposal

## Why

`mdnsview` can only be driven from the keyboard. Browsing a long device list or a large raw-records view is quicker for casual users when they can point at a row and use the scroll wheel. Mouse capture turns off the terminal's own text selection, which people use to copy addresses and hostnames, so it has to be easy to turn off.

## What Changes

- Clicking a row in the device/service-type pane or the services/instances pane focuses that pane and selects the row. Clicking the details pane focuses it.
- The scroll wheel acts on the pane under the pointer: in the two list panes it moves the selection, in the details pane it scrolls.
- Mouse capture is on at startup by default. A new key `m` turns it off and on at runtime, so the terminal's native text selection works again. The status bar shows the current mouse state.
- New configuration setting `mouse` (boolean, default `true`) sets whether capture is on at startup.
- On exit, including exit caused by an error or panic, the terminal is restored with mouse capture switched off.
- Out of scope: double-click actions, dragging pane borders, clickable status-bar hints, and right-click menus.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `browser-ui`:
  - adds requirements for mouse selection, wheel scrolling and the mouse toggle
  - the keyboard navigation requirement gains the `m` key and restoring mouse capture on exit
  - the status bar requirement gains the mouse state
- `app-configuration`: the defaults and supported settings gain the `mouse` setting.

## Impact

- Code: `src/ui` (event loop, key and mouse handling in `App`, hit-testing against the rendered layout, terminal setup and restore) and `src/config.rs` (new field).
- Docs: README key table and configuration example.
- No new dependencies; crossterm's mouse events and the `EnableMouseCapture` / `DisableMouseCapture` commands are already available through ratatui.
