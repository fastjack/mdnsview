//! A backend for platforms without a supported mDNS daemon.

use tokio::sync::mpsc;

use super::{BoxFuture, SystemBackend};
use crate::model::record::{Observation, Source, SourceState};

// Used on platforms without a daemon backend, and in tests.
#[cfg_attr(any(target_os = "linux", target_os = "macos"), allow(dead_code))]
pub struct NullBackend;

impl SystemBackend for NullBackend {
    fn name(&self) -> &'static str {
        "none"
    }

    fn run(self: Box<Self>, observations: mpsc::Sender<Observation>) -> BoxFuture {
        Box::pin(async move {
            let _ = observations
                .send(Observation::SourceStatus {
                    source: Source::System,
                    state: SourceState::Unavailable("no supported mDNS daemon".into()),
                })
                .await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::snapshot::Status;
    use crate::model::store::spawn;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn store_runs_with_null_backend() {
        let (mut handle, _task) = spawn(Duration::from_secs(300), None);
        Box::new(NullBackend).run(handle.observations.clone()).await;
        tokio::time::advance(Duration::from_millis(300)).await;
        handle.snapshots.changed().await.unwrap();
        let status: Status = handle.snapshots.borrow().status.clone();
        assert!(matches!(status.system, SourceState::Unavailable(_)));
        assert!(handle.snapshots.borrow().devices.is_empty());
    }
}
