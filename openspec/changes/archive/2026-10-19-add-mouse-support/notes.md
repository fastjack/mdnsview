# Verification notes

Checked on 2026-10-19 on the development machine (Arch Linux, avahi-daemon 0.9-rc5, Rust 1.98.1) against the office LAN, inside tmux 150×30.

- **Automated:** `cargo clippy --all-targets -- -D warnings` is clean. `cargo test` passes 93 tests, with 2 ignored live-network tests.
  - New tests cover the hit map of a scrolled list, clicks (device, service, below the last row, border, scrolled list, details), wheel over the lists and the details, ignored mouse events, the `m` toggle and its status-bar segment, `m` typed in the filter, and the `mouse` config setting.
- **Capture state:** read from tmux's `#{mouse_any_flag}` for the pane.
  - Startup with defaults: on. After `m`: off. After `m` again: on.
  - After `q`: off, exit status 0.
  - After a forced panic in a scratch build (panic on F12) while capture was on: off. The alternate screen was left and the panic message printed normally.
  - `mouse = false` in the config: starts off, and `m` turns it on.
- **Clicks and wheel:** injected into the pane as SGR mouse sequences with `tmux send-keys -l`.
  - Clicking the 4th device row selected it.
  - Three wheel-down steps over the device list moved the selection down three entries.
- **Keyboard unchanged:** the sequence `j j l d tab /brother enter` produced the same screen with capture on and with capture off. The status bar showed `mouse on` and `mouse off` respectively.
- **Not checked by hand:** a real mouse drag selecting text after `m`. With capture off the terminal receives no mouse-reporting request (flag 0), so selection is handled by the terminal itself.
