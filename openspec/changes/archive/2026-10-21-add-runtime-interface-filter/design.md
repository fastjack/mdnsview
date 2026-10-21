# Design

## Context

See proposal.md for motivation and specs/ for the required behaviour. Relevant facts about the current code:

- `wire::interfaces::Selector` (src/wire/interfaces.rs) turns `sys::list_interfaces()` into the selected interfaces: up, multicast, not loopback, then include / exclude globs. It is used only by the wire backend.
- `WireBackend` (src/wire/backend.rs) polls interfaces every 5 s in its `select!` loop (`sync_interfaces`), joins / leaves groups by diffing `joined`, drops packets from interfaces it has not joined, and on change calls `scheduler.restart_enumeration` and sends `Observation::Interfaces(names)`.
- When the multicast socket cannot be opened, no backend task runs, so nothing polls interfaces and the status shows `0 interfaces`.
- The avahi backend (src/backend/avahi.rs) browses with `IF_UNSPEC`. It already tracks, per instance, the set of `(ifindex, protocol)` scopes avahi reports it on, and emits `SystemRemoved` when the last scope goes away. It resolves interface names through `GetNetworkInterfaceNameByIndex` and caches them.
- The store's `system_removed` → `release_system` clears `held_by_system` and sets `goodbye` when no live wire copy exists, which makes the entry **left**.
- The UI talks back to the store only through `StoreHandle::retain`. `App` is pure state, tested without a terminal; the filter prompt (`editing_filter`) is the existing example of a mode that captures keys.

## Goals / Non-Goals

**Goals:**
- One place computes which interfaces are in use, shared by the wire backend, the avahi backend and the status bar.
- Selection changes reach the sockets within one second, not at the next 5 s poll.
- Picker logic lives in `App` and is unit-tested like the filter prompt.

**Non-Goals:**
- Per-interface avahi browsers (re-browsing with a concrete interface index). Filtering avahi's `IF_UNSPEC` results is enough.
- Tracking every interface a record was received on. `StoredRecord::iface` stays a single value.

## Decisions

### D1. `InterfaceSelection` travels on a `watch` channel from the UI

- New type `InterfaceSelection { All, Only(BTreeSet<String>) }` in `src/model/record.rs`, next to the other types shared between sources, store and UI.
- `main` creates `watch::channel(InterfaceSelection::All)`. The UI gets the sender; discovery gets receivers.
- A `watch` fits: only the latest selection matters, and receivers wake immediately on change.
- Alternative considered: send the selection through the store and let it forward to the backends. Rejected because the store does not talk to the avahi backend and would just relay.

### D2. An interface monitor task computes eligible and active interfaces

- New `wire::monitor` task, spawned by `main` before both backends and regardless of whether the socket opened. It owns the `Selector` and a `watch::Receiver<InterfaceSelection>`.
- It recomputes on a 5 s poll tick and whenever the selection changes:
  - `eligible = selector.select(&sys::list_interfaces()?)` (unchanged rules)
  - `active = eligible` filtered by the selection (`All` → all; `Only(names)` → names in the set)
- It publishes `watch<InterfaceSet { eligible: Vec<Interface>, active: Vec<Interface> }>` and, on every change, sends `Observation::Interfaces { eligible, active, selection }` to the store.
- `main` computes the first value synchronously before spawning anything, so the backends never see an empty "not known yet" set at startup and avahi results are filtered from the first signal.
- Filtering active from eligible is a pure function, `Selector::active(&eligible, &selection)`, with unit tests for "selected interface absent", "new interface with explicit selection" and "new interface with All".
- Alternative considered: keep polling inside `WireBackend` and add the selection there. Rejected because avahi filtering, the picker and system-only mode all need the eligible list when the wire backend is not running.

### D3. `WireBackend` follows the monitor instead of polling

- `WireBackend::open` takes a `watch::Receiver<InterfaceSet>` instead of a `Selector`. `sync_interfaces` diffs `joined` against `active` exactly as today; only the source of `wanted` changes.
- The 5 s `poll.tick()` arm is replaced by `interfaces.changed()`. On change it syncs, restarts enumeration and stops sending `Observation::Interfaces` (the monitor now does).
- Packet filtering stays as it is: packets on a non-joined interface index are dropped, which covers deselected interfaces.

### D4. Avahi filters and replays by interface name

- `SystemBackend::run` gains a `watch::Receiver<InterfaceSet>` parameter; `NullBackend` ignores it.
- The avahi `State` keeps, per `(instance key, scope)`, the observations last produced for it (the PTR from `ItemNew` and the SRV / TXT / A from `Found`). Scope → interface name comes from the existing name cache.
- A scope is *in use* when its interface name is in `active`, or its name is unknown (`IF_UNSPEC` / lookup failed), matching "records without an interface are kept".
- Signal handling:
  - `ItemNew` / `Found` on a scope not in use: store the observations, emit nothing.
  - `ItemRemove`: emit `SystemRemoved` only when the instance had an in-use scope and has none left (today: when no scope is left).
- On `InterfaceSet` change, for every instance compare in-use scopes before and after:
  - had some, now none → emit the new `Observation::SystemReleased { instance, at }`
  - had none, now some → replay the cached observations of the newly in-use scopes with fresh `at` / `wall`
- The scope bookkeeping is a plain struct without D-Bus types, so the before/after logic is unit-tested.
- Alternative considered: free and recreate resolvers on reselect so avahi resends `Found`. Rejected: slower, more D-Bus traffic, and avahi may answer from cache anyway.

### D5. Release without goodbye in the store

- `Observation::SystemReleased { instance, at }` runs the same key collection as `system_removed` (instance records, PTRs to it, then address records no longer used by a held SRV), but calls a release that clears `held_by_system` and bumps `last_seen` without setting `goodbye`.
- Result: an avahi-only entry turns **stale** at once and is removed after the device timeout; an entry with a live wire copy stays live until that copy expires. Neither is shown as **left**.
- `system_removed` keeps its current behaviour.

### D6. Status carries the interface state for the UI

- `Status::interfaces: Vec<String>` becomes:
  - `active: Vec<String>`
  - `eligible: Vec<InterfaceInfo { name, addresses }>`
  - `selection: InterfaceSelection`
- The status bar prints `N interfaces` for `All`, and `N of M interfaces` (M = eligible count) for `Only`.
- In system-only mode the count now reflects the interfaces avahi results are accepted from, instead of always `0`.

### D7. Picker state in `App`

- `App` gains `picker: Option<Picker>`, with `Picker { cursor, all: bool, checked: BTreeSet<String>, error: bool }`, initialised from `snapshot.status.selection` when `i` opens it.
- Picker rows are computed on demand from the snapshot: "All interfaces", then the union of eligible names and checked names, sorted. Checked names that are not eligible are marked unavailable.
- While `picker` is `Some`, `on_key` routes every key to the picker, as `editing_filter` does for the filter prompt:
  - `j`/`k`/arrows move
  - `space` toggles
  - `a` checks "All interfaces"
  - `enter` applies, or sets `error` when nothing is checked
  - `esc` closes
  - anything else is ignored, including `q`
- Applying sets `app.selection_request = Some(selection)`. `ui::run` sends it on the watch sender after handling the event, mirroring how `mouse` and `retained_ids` are propagated today.
- Mouse events are ignored while the picker is open, so a click cannot change the selection underneath it.
- `view::render` draws the picker last, as a centred bordered list over the panes (`Clear` + `List`). The status bar hints switch to the picker keys while it is open, and show the "check at least one" message when `error` is set.

## Risks / Trade-offs

- [The status bar count in system-only mode changes from `0` to the eligible count] → Matches what is now actually filtered. Covered by a view test.
- [Avahi results from `veth*` and other excluded interfaces disappear for existing users] → Intentional and listed as a behaviour change in the proposal; mention it in the README.
- [A record seen on two interfaces keeps only one `iface` in the raw view] → Unchanged from today. Avahi in-use decisions use the backend's own per-scope bookkeeping, not `StoredRecord::iface`, so liveness stays correct.
- [Replayed avahi observations look "just seen" although avahi learned them earlier] → Correct for liveness, since avahi still holds them. First-seen is kept when the store still has the record.
- [Wire entries on a reselected interface reappear only as hosts answer] → Enumeration is restarted on every selection change, so most answers arrive within a few seconds.
- [Interface names, not indices, identify the selection] → Survives an interface being recreated with a new index (USB adapters, VPNs), which is the common case.
