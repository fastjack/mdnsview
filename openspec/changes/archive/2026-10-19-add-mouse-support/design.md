# Design

## Context

See proposal.md for motivation and specs/ for the required behaviour. Relevant facts about the current code:

- `ui::run` (src/ui/mod.rs) loops: draw → `select!` over the crossterm `EventStream`, snapshot changes and a 1 s tick → apply to `App`. Only `Event::Key` is handled.
- `App` (src/ui/app.rs) holds all UI state and is unit-tested without a terminal. Selection moves go through `move_selection(delta)`, which acts on the focused pane.
- `view::render` (src/ui/view.rs) computes the pane layout every frame. It builds a fresh `ListState` per list on each frame, and ratatui picks the scroll offset that keeps the selection visible. That offset is not kept anywhere today.
- The terminal is set up with `ratatui::try_init()` and restored by a drop guard calling `ratatui::restore()`; ratatui's panic hook also calls `restore()`. Neither touches mouse capture.

## Goals / Non-Goals

**Goals:**
- Map a click to the exact entry under the pointer, including in scrolled lists.
- Keep `App` free of terminal I/O, so mouse behaviour is unit-tested like key handling.
- Never leave the user's terminal in mouse-reporting mode, including after a panic.

**Non-Goals:**
- Hover effects, double-click, dragging, and making the status bar clickable.

## Decisions

### D1. `render` returns a hit map of the frame it drew

- `view::render` returns a small `HitMap`:
  - the outer rectangle of each pane
  - the inner (bordered-off) rectangle of the two list panes
  - the scroll offset ratatui used for each list, read from the `ListState` after `render_stateful_widget`
- `ui::run` keeps the hit map of the last frame and passes it to `App::on_mouse(event, &hit_map)`.
- A click is always read against the last drawn frame. The loop redraws after every snapshot change, before handling the next event, so the rows the user sees are the rows the hit map describes.
- Alternative considered: keep persistent `ListState`s in `App`. Rejected because it mixes widget state into `App` and still needs the pane rectangles from rendering.

### D2. Mouse handling in `App`

`App::on_mouse(MouseEvent, &HitMap)` handles these events:

- **Left button down in a list pane:**
  - focuses that pane
  - row index = `offset + (y - inner.y)`
  - if the index refers to an existing entry, selects it through the same code path as keyboard moves, so the middle pane resets and selection fixing work the same way
  - a click on the border or below the last row only focuses
- **Left button down in the details pane:** focuses it.
- **Scroll up/down:** finds the pane under the pointer and moves its selection by ∓1, or scrolls the details by ∓1 line. Focus is unchanged.

To support wheel scrolling over a pane without focus, `move_selection(delta)` becomes `move_in(pane, delta)`; keyboard handling passes the focused pane. All other mouse events (release, drag, move, right/middle button) are ignored.

### D3. Mouse capture state and the `m` key

- `App` gains `mouse: bool`, initialised from `Config::mouse`. The `m` key flips it; the key is ignored while the filter prompt is being edited, where `m` is ordinary text.
- `ui::run` keeps track of the capture state it last applied to the terminal. After each event it compares that with `app.mouse` and runs `EnableMouseCapture` or `DisableMouseCapture` when they differ.
- The terminal I/O stays in the loop and `App` stays pure.
- While capture is off the terminal sends no mouse events, so nothing needs filtering.

### D4. Releasing capture on every exit path

- The drop guard runs `DisableMouseCapture` before `ratatui::restore()`. It does this unconditionally, since disabling is harmless when capture is already off.
- After `try_init`, `ui::run` installs its own panic hook around the existing one. The hook writes `DisableMouseCapture` to stdout, then calls the previous hook (ratatui's restore + the default message).
- Errors returned from the loop drop the guard, so they go through the same path.

### D5. Status bar and configuration

- The status bar adds a `mouse on` / `mouse off` segment, and the key hints gain `m mouse`.
- `Config` gains `mouse: bool` with default `true`. `deny_unknown_fields` already covers typos.
- The README configuration example gains `mouse = true`; the existing test that parses the README example keeps it accurate.

## Risks / Trade-offs

- [Terminals without mouse reporting] → crossterm's enable command is harmless where unsupported; keyboard use is unchanged.
- [Users expect native selection to keep working] → `m` toggles capture, the status bar shows the state, and `mouse = false` disables it at startup. Most terminals also allow Shift+drag selection while capture is on; the README mentions both.
- [A click arrives while the list is being rebuilt by a snapshot change] → impossible by construction (D1): each loop iteration handles one event against the frame drawn just before it.
