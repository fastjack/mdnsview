//! The interactive terminal UI.

pub mod app;
pub mod view;

#[cfg(test)]
mod tests;

use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use ratatui::crossterm::event::{Event, EventStream};
use tokio::time::MissedTickBehavior;

use crate::model::store::StoreHandle;
use app::App;

const TICK: Duration = Duration::from_secs(1);

/// Restores the terminal when dropped, including on early returns.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// Runs the UI until the user quits.
pub async fn run(mut store: StoreHandle, system_name: &str) -> Result<()> {
    // `init` also installs a panic hook that restores the terminal.
    let mut terminal = ratatui::try_init()?;
    let _guard = TerminalGuard;
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut app = App::new(store.snapshots.borrow_and_update().clone());
    let mut retained = app.retained_ids();
    let _ = store.retain.send(retained.clone());

    loop {
        terminal.draw(|frame| view::render(frame, &app, system_name))?;
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) => app.on_key(key),
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
        let ids = app.retained_ids();
        if ids != retained {
            retained = ids;
            let _ = store.retain.send(retained.clone());
        }
    }
    Ok(())
}
