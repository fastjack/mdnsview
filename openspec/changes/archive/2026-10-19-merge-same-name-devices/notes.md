# Verification notes

Checked on 2026-10-19 on the development machine (Arch Linux, avahi-daemon 0.9-rc5, Rust 1.98.1).

## Automated

- `cargo clippy --all-targets -- -D warnings` is clean.
- `cargo test` passes 111 tests (2 live-network tests ignored).
- New model tests:
  - the Hyperion case (one device, 9 services, union of addresses, every service's `device_id` = `hyperion.local`)
  - same name with different addresses stays separate
  - merging waits for a shared address, and the larger part keeps its id
  - three hosts merge transitively
  - the unresolved bucket is never merged
  - last-seen is the latest host
  - a merged device is live while one host is live
  - retention through a member host key
  - a cache-flushed address change splits the device, and the primary keeps its id
  - a fully expired merged device stays merged
  - a silent host leaves a live merged device
  - a selected primary host is kept, with its services, while the other host answers
  - a retained, gone merged device is kept whole until released
- New UI tests:
  - the selection follows a merge
  - the selection and the selected service survive a split
  - `/1e8da75b` keeps the merged device
  - the service view shows a `Device` line only for multi-host devices
  - the device details list all host names

## Office LAN (tmux, 170×40)

- "Hyperion" is listed once. Selecting it shows 9 services (RAOP, `_abm-mh-v2`, `_net-assistant`, AirPlay, companion link, VNC, SFTP, SMB, SSH).
- Its service view shows `Device Hyperion.local, 1e8da75b-81ab-43ae-b199-5d0fddbc5c59.local` and the merged addresses `10.10.132.147` and `fe80::c05:d0d4:84b1:b726`.

## Decisions made during implementation

- **Held hosts:** a member host is kept while its own host key or one of its services is retained, and a held host keeps its services (design D3). Without this, a selected merged device would change id, or split, when its primary host went silent while the other host still answered.
- **Where host names are shown:** the middle pane always selects a service, so a merged device's host names are also shown as a `Device` line in the service view (agreed with the user; spec and design updated).
