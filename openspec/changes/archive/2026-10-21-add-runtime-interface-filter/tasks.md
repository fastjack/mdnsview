# Tasks

## 1. Selection type and interface monitor

- [x] 1.1 Add `InterfaceSelection { All, Only(BTreeSet<String>) }` to `src/model/record.rs`; verify it builds with `cargo check`
- [x] 1.2 Add `Selector::active(&eligible, &selection)` in `src/wire/interfaces.rs`; verify unit tests: `All` returns every eligible interface, `Only` keeps only named ones, a selected name that is absent yields nothing for it, a newly eligible interface is excluded under `Only` and included under `All`, an ineligible name in `Only` (e.g. `veth1a2b`) is never returned
- [x] 1.3 Add the `wire::monitor` task: computes `InterfaceSet { eligible, active }` on a 5 s poll and immediately on selection change, publishes it on a `watch`, and sends `Observation::Interfaces { eligible, active, selection }` to the store on every change; expose a synchronous `initial()` for startup. Verify a unit test with a fake interface lister that a selection change produces a new `InterfaceSet` without waiting for the poll

## 2. Store and status

- [x] 2.1 Change `Observation::Interfaces` and `Status` to carry `active`, `eligible` (name + addresses) and `selection` (design D6); update existing tests and the status bar to use `active`; verify `cargo test` passes with the status bar still showing `1 interface` in the existing UI test
- [x] 2.2 Add `Observation::SystemReleased { instance, at }` and a release path that clears `held_by_system` without setting `goodbye` (design D5); verify store unit tests: an avahi-only service becomes stale (not left) and is removed after the device timeout; a service with a live wire copy stays live; `SystemRemoved` still marks it left

## 3. Wire backend follows the monitor

- [x] 3.1 Make `WireBackend::open` take the `InterfaceSet` receiver instead of a `Selector`; replace the poll arm with `interfaces.changed()`, which syncs groups and restarts enumeration; stop sending `Observation::Interfaces` from the backend. Update the ignored `live_wire` test to build a monitor. Verify `cargo test` passes and `cargo test live_wire -- --ignored --nocapture` still receives records

## 4. Avahi filtering by interface

- [x] 4.1 Add a D-Bus-free scope book to the avahi backend: per `(instance, scope)` cached observations, an in-use check by interface name (unknown name counts as in use), and a `diff(old_active, new_active)` returning instances to release and observations to replay. Verify unit tests for the spec scenarios: excluded `veth*` scope emits nothing; a service on a deselected interface is released; a service on both wlan0 and enp3s0 stays when wlan0 is deselected; reselecting replays the cached SRV / TXT / A / PTR
- [x] 4.2 Wire the scope book into `handle_signal` (`ItemNew`, `Found`, `ItemRemove` emit only for in-use scopes; `SystemRemoved` only when the last in-use scope goes) and add an `InterfaceSet` arm to the avahi run loop that emits releases and replays; add the receiver parameter to `SystemBackend::run` and ignore it in `NullBackend`. Verify `cargo test` and the ignored avahi live test still list services

## 5. Picker UI

- [x] 5.1 Add `Picker` state and key routing to `App` (design D7): `i` opens it from the current selection; `j`/`k`/arrows, `space`, `a`, `enter`, `esc`; other keys including `q` ignored; mouse ignored while open; applying sets `selection_request`; nothing checked sets the error. Verify unit tests for every scenario in the browser-ui "Interface picker", "Choosing interfaces in the picker" and "Applying the interface choice" requirements, and that `i` typed into the filter prompt is text
- [x] 5.2 Render the picker as a centred overlay with checkboxes, addresses and an "unavailable" mark for absent selected interfaces; switch the status bar hints while open and show the "check at least one" hint on error; show `N of M interfaces` for a restricted selection and add `i interfaces` to the normal hints. Verify `TestBackend` tests: picker contents (veth not listed, absent wlan0 marked unavailable), `2 interfaces` for All, `1 of 3 interfaces` for a restricted selection
- [x] 5.3 In `main` and `ui::run`: create the selection `watch`, compute the initial `InterfaceSet`, spawn the monitor before both backends (also when the socket fails), pass receivers to both backends, and send `selection_request` from the UI loop. Verify `cargo build` and that system-only mode (`wire` socket blocked) still starts and shows a non-zero interface count

## 6. Documentation and integration

- [x] 6.1 Update the README: `i` in the key table, a short "Choosing interfaces" section (all / one / several, eligible = config rules, deselected entries age out, not saved), and a note that include / exclude now also apply to avahi results; verify the key table matches `App::on_key` and the README TOML example parse test still passes
- [x] 6.2 Run `cargo clippy --all-targets -- -D warnings` and `cargo test`; then on a host with two networks, check in tmux: restricting to one interface stops new devices from the other appearing and lets its devices go stale, "All" brings avahi entries back within a second, and an explicit selection survives unplugging and replugging its interface. Record the results in the change notes

## Workflow follow-up

- Archive the change after review with `/opsx:archive`.
