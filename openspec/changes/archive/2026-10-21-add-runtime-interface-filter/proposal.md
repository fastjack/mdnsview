# Proposal

## Why

On a machine with several networks (Wi-Fi plus Ethernet, a VPN, a docker bridge), `mdnsview` mixes the devices from all of them into one list. To look at a single network today, users have to edit `interfaces.include` in the configuration file and restart, which also loses everything discovered so far. Choosing interfaces while the application runs makes it quick to focus on one network, or a few, and to go back to all of them.

## What Changes

- A new key `i` opens an interface picker over the panes. It lists the eligible interfaces: those that pass the built-in rules (up, multicast-capable, not loopback) and the configured `interfaces.include` / `interfaces.exclude` patterns, including the default `veth*` exclusion. Interfaces outside that set are never offered.
- In the picker the user can choose **all** interfaces (the default), **one**, or **several**. Changes take effect when applied with `enter`; `esc` cancels.
- The chosen interfaces change what is captured, not only what is shown:
  - the multicast socket joins the mDNS groups and sends queries only on the chosen interfaces
  - records the OS daemon (avahi) reports on other interfaces are not used
- Entries discovered on deselected interfaces are no longer refreshed and age out through the normal expiry and device timeout. Avahi-held entries from those interfaces are released rather than marked as having left. Re-selecting an interface brings avahi's entries for it back immediately; wire entries return as hosts answer the restarted enumeration.
- "All" follows interface changes: an eligible interface that comes up later is used automatically. An explicit selection does not grow by itself. A selected interface that goes away stays selected and is used again when it comes back.
- The status bar shows how many of the eligible interfaces are in use when the selection is restricted (for example `1 of 3 interfaces`), and gains an `i` hint.
- Behaviour change: the configured include / exclude patterns now also apply to records reported by avahi, not only to the multicast socket. Previously, avahi results from an excluded interface such as `veth*` were still listed.
- The runtime selection is not saved; every start uses all eligible interfaces.
- Out of scope: a command-line option for the selection, saving the selection to the configuration file, mouse interaction inside the picker, and a view-only filter that hides entries without changing capture.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `mdns-discovery`:
  - interface selection gains a runtime selection within the eligible interfaces
  - interface change detection follows the runtime selection
  - the OS daemon results are limited to the interfaces in use
- `browser-ui`:
  - adds a requirement for the interface picker
  - keyboard navigation gains the `i` key
  - the status bar shows a restricted interface count and the `i` hint

## Impact

- Code:
  - `src/wire`: the interface selector separates eligible from active interfaces; the backend follows a runtime selection and restarts enumeration when it changes.
  - `src/backend`: the `SystemBackend` trait gains the interface selection; the avahi backend filters and replays its items by interface.
  - `src/model`: `Observation::Interfaces` and `Status` carry eligible and active interfaces; the store gains a release-without-goodbye path for daemon entries.
  - `src/ui`: picker state and key handling in `App`, an overlay in `view`, and a channel from the UI back to discovery.
  - `src/main.rs`: wiring the selection channel and an interface monitor that also runs in system-only mode.
- Docs: README key table and a paragraph on choosing interfaces.
- No new dependencies.
