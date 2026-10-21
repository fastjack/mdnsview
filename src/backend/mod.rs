//! OS mDNS daemon backends: the coarse first source of discovery data.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::{mpsc, watch};

use crate::model::record::Observation;
use crate::wire::monitor::InterfaceSet;

#[cfg(target_os = "linux")]
pub mod avahi;
pub mod null;

pub type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// A discovery source backed by the operating system's mDNS daemon.
///
/// `run` reports its own [`Observation::SourceStatus`]: `Active` once it is
/// connected, `Unavailable` if the daemon cannot be used. It returns when the
/// daemon goes away or the observation channel closes. Only results on the
/// interfaces `interfaces` marks active are reported.
pub trait SystemBackend: Send {
    /// Short name for the status bar, e.g. "avahi".
    fn name(&self) -> &'static str;
    fn run(
        self: Box<Self>,
        observations: mpsc::Sender<Observation>,
        interfaces: watch::Receiver<InterfaceSet>,
    ) -> BoxFuture;
}

/// The backend for this platform.
pub fn platform() -> Box<dyn SystemBackend> {
    #[cfg(target_os = "linux")]
    {
        Box::new(avahi::AvahiBackend)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Box::new(null::NullBackend)
    }
}
