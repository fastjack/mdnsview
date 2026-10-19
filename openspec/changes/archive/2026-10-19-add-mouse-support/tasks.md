# Tasks

## 1. Configuration

- [x] 1.1 Add `mouse: bool` (default `true`) to `Config`; verify unit tests: no file → mouse on, `mouse = false` → off, a partial file keeps mouse on
- [x] 1.2 Add `mouse = true` with a comment to the README configuration example; verify the README example parse test still passes and asserts `mouse` is true

## 2. Hit map and mouse handling

- [x] 2.1 Make `view::render` return a `HitMap` (outer pane rects, inner list rects, list scroll offsets read back from `ListState`); verify with a `TestBackend` test that the offsets and rectangles match a scrolled device list (selection at entry 20 of 40 in a short terminal)
- [x] 2.2 Refactor `App::move_selection` into `move_in(pane, delta)`, keeping key behaviour unchanged; verify all existing UI tests still pass
- [x] 2.3 Implement `App::on_mouse` for left clicks (select the row and focus its pane; a click on empty area or the border only focuses; a details click focuses); verify unit tests for the click-a-device, click-a-service, click-below-last-row and click-in-scrolled-list scenarios
- [x] 2.4 Implement wheel handling: list panes move the selection by one, the details pane scrolls by one line, focus is unchanged, other mouse events are ignored; verify unit tests for the wheel-over-device-list and wheel-over-details scenarios

## 3. Mouse capture toggle and terminal handling

- [x] 3.1 Add `App::mouse`, initialised from config, toggled by `m` (ignored while editing the filter, where `m` is text); verify unit tests for toggling and for typing `m` into the filter
- [x] 3.2 Show `mouse on` / `mouse off` in the status bar and add `m mouse` to the key hints; verify a `TestBackend` test for both states
- [x] 3.3 In `ui::run`: handle `Event::Mouse` via `on_mouse` with the last hit map, enable capture at startup when configured, apply `Enable/DisableMouseCapture` when `app.mouse` changes, disable capture in the drop guard, and wrap the panic hook to disable capture first. Verify manually in tmux:
  - clicking rows selects them, and the wheel scrolls
  - `m` turns capture off, so a drag selects terminal text
  - after `q`, and after a forced panic in a scratch build, the shell receives no mouse escape sequences

## 4. Documentation and integration

- [x] 4.1 Document mouse usage in the README: click, wheel, the `m` key, Shift+drag in most terminals, and `mouse = false`; verify the documented key table matches `App::on_key`
- [x] 4.2 Run `cargo clippy --all-targets -- -D warnings` and `cargo test`, and check against the real LAN in tmux that keyboard use is unchanged with capture both on and off; record the results in the change notes

## Workflow follow-up

- Archive the change after review with `/opsx:archive`.
