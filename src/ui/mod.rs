//! The interactive terminal UI.

pub mod app;
pub mod view;

#[cfg(test)]
mod tests;

use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream};
use ratatui::crossterm::execute;
use tokio::time::MissedTickBehavior;

use crate::model::store::StoreHandle;
use app::App;

const TICK: Duration = Duration::from_secs(1);

/// Restores the terminal when dropped, including on early returns.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        release_mouse();
        ratatui::restore();
    }
}

/// Turns mouse reporting off; harmless when it is already off.
fn release_mouse() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
}

fn set_mouse_capture(on: bool) -> std::io::Result<()> {
    if on {
        execute!(std::io::stdout(), EnableMouseCapture)
    } else {
        execute!(std::io::stdout(), DisableMouseCapture)
    }
}

/// Runs the UI until the user quits.
pub async fn run(mut store: StoreHandle, system_name: &str, mouse: bool) -> Result<()> {
    // `init` also installs a panic hook that restores the terminal, but it
    // leaves mouse capture on; wrap it so a panic releases the mouse first.
    let mut terminal = ratatui::try_init()?;
    let _guard = TerminalGuard;
    let ratatui_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        release_mouse();
        ratatui_hook(info);
    }));
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut app = App::new(store.snapshots.borrow_and_update().clone());
    app.mouse = mouse;
    let mut mouse_captured = mouse;
    set_mouse_capture(mouse_captured)?;
    let mut hits = app::HitMap::default();
    let mut retained = app.retained_ids();
    let _ = store.retain.send(retained.clone());

    loop {
        terminal.draw(|frame| {
            hits = view::render(frame, &app, system_name);
        })?;
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) => app.on_key(key),
                Some(Ok(Event::Mouse(event))) => app.on_mouse(event, &hits),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => break,
            },
            changed = store.snapshots.changed() => {
                if changed.is_err() {
                    break;
                }
                app.set_snapshot(store.snapshots.borrow_and_update().clone());
            }
            _ = tick.tick() => {}
        }
        if app.quit {
            break;
        }
        if app.mouse != mouse_captured {
            mouse_captured = app.mouse;
            set_mouse_capture(mouse_captured)?;
        }
        let ids = app.retained_ids();
        if ids != retained {
            retained = ids;
            let _ = store.retain.send(retained.clone());
        }
    }
    Ok(())
}
