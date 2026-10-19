# Proposal

## Why

Some machines advertise under more than one hostname, so `mdnsview` lists them twice under the same name. On the office LAN, the Mac "Hyperion" uses `Hyperion.local` for 8 services and a generated `1e8da75b-…5c59.local` for its device-management service. Both hostnames resolve to the same IPv4 and IPv6 addresses. Two identical "Hyperion" rows confuse casual users, and the second row holds only one of the machine's services. v1 deliberately did not merge hostnames; now that labels come from instance names, the label plus a shared address identifies these duplicates reliably.

## What Changes

- Two hostnames are shown as **one device** when their labels match (case-insensitive) **and** they share at least one IP address. Merging is transitive and always on, with no setting.
- A merged device:
  - lists the services of all its hostnames
  - shows the union of their addresses
  - is last seen when its most recently seen hostname was
  - is live while any of its records is live
- Hosts with the same label but no common address stay separate, and so do hosts whose addresses are not known yet. A merged device splits again when the hosts no longer share an address.
- A hostname of a merged device that has not been seen for longer than the device timeout is dropped from the device, while the rest stays.
- The details pane shows every hostname of a merged device: a "Device" line in the service view, and a hostname list in the device details.
- When the selected device merges with another or splits, the selection stays on the device that now contains the selected hostname, and retention of a selected entry keeps working across merges.
- **BREAKING (spec):** replaces the v1 rule "different host names SHALL NOT be merged automatically, even when they share an address".
- Out of scope: merging by address alone (sleep proxies answer for several machines), merging hosts with different labels, and manual merge/unmerge.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `device-inventory`:
  - "Devices identified by hostname" changes to allow merging
  - new requirements define when hostnames merge, what a merged device contains, and how a stale hostname leaves a merged device
- `browser-ui`:
  - new requirement: the selection follows merges and splits
  - new requirement: device details list all hostnames

## Impact

- Code: `src/model/snapshot.rs`:
  - per-host derivation, then grouping into devices
  - `Device` gains its member hosts
  - device id = primary host
- Code: `src/model/store.rs`: GC works per member host inside a merged device.
- Code: `src/ui/app.rs`: the filter matches every hostname, and selection and retention resolve hostnames to their device.
- Code: `src/ui/view.rs`: the device details list all hostnames.
- Docs: README usage section.
- No new dependencies and no configuration changes.
