//! OS mDNS daemon backends: the coarse first source of discovery data.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::model::record::Observation;

#[cfg(target_os = "linux")]
pub mod avahi;
#[cfg(target_os = "macos")]
pub mod dnssd;
pub mod null;

pub type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// A discovery source backed by the operating system's mDNS daemon.
///
/// `run` reports its own [`Observation::SourceStatus`]: `Active` once it is
/// connected, `Unavailable` if the daemon cannot be used. It returns when the
/// daemon goes away or the observation channel closes.
pub trait SystemBackend: Send {
    /// Short name for the status bar, e.g. "avahi".
    fn name(&self) -> &'static str;
    fn run(self: Box<Self>, observations: mpsc::Sender<Observation>) -> BoxFuture;
}

/// The backend for this platform.
pub fn platform() -> Box<dyn SystemBackend> {
    #[cfg(target_os = "linux")]
    {
        Box::new(avahi::AvahiBackend)
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(dnssd::DnsSdBackend)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Box::new(null::NullBackend)
    }
}
