# Verification notes

Checked on 2026-10-21 on the development machine (Arch Linux, avahi-daemon 0.9-rc5, Rust 1.98.1, tmux 3.8, 200×30). Eligible interfaces were `enp4s0f1u1` (link-local IPv6 only), `tailscale0` and `wlp5s0` (home LAN, 29 devices).

- **Automated:** `cargo clippy --all-targets -- -D warnings` is clean. `cargo test` passes 156 tests, with 2 ignored live-network tests.
  - New tests cover:
    - `Selector::active` and the interface monitor, including a selection change applied without waiting for the poll
    - release without goodbye in the store
    - the avahi scope book: excluded interface, deselect, two interfaces, replay, removal after release
    - every picker scenario, the picker overlay, and the `N of M interfaces` status bar
- **Live tests:** `live_wire` received records with the monitor in place. `live_avahi` listed 404 records on `wlp5s0` with all eligible interfaces in use.
- **Picker in tmux:**
  - `i` listed "All interfaces" (checked) and the three eligible interfaces with their addresses. No `veth*`, `docker*` or `lo` was listed.
  - Selecting only `tailscale0` and pressing `enter` changed the status bar to `1 of 3 interfaces`. After 20 s every LAN device showed `21s` instead of `now`, so nothing was being refreshed.
  - `i a enter` changed it back to `3 interfaces`, and within 1.5 s every device showed `now` again.
- **Not checked live:**
  - **System-only mode:** the multicast socket could not be made to fail while avahi holds port 5353 with address sharing. In `main`, the monitor is spawned before `WireBackend::open`, whatever its result.
  - **Unplugging and replugging** a selected interface: this needs physical access. It is covered by the `Selector::active` unit tests, and the 5 s monitor poll is unchanged from the old backend poll.
  - Whether new devices from a deselected network stay out. The live run only showed that existing ones stopped refreshing.
