mod backend;
mod config;
mod model;
mod ui;
mod wire;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use clap::Parser;
use tokio::sync::mpsc;
use tracing::{info, warn};

use config::Config;
use model::record::{Observation, Source, SourceState};
use wire::backend::WireBackend;
use wire::interfaces::Selector;

/// How long to wait for the OS daemon when it is the only possible source.
const SYSTEM_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Browse mDNS / DNS-SD devices and services on the local network.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Configuration file to use instead of the platform default.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::load(args.config.as_deref())?;
    let _log = init_logging();
    tokio::runtime::Runtime::new()?.block_on(run(config))
}

/// Logs go to a file, never to the terminal the UI draws on.
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let dirs = directories::ProjectDirs::from("", "", "mdnsview")?;
    let dir = dirs.state_dir().unwrap_or(dirs.cache_dir()).to_path_buf();
    std::fs::create_dir_all(&dir).ok()?;
    let appender = tracing_appender::rolling::never(&dir, "mdnsview.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Some(guard)
}

async fn run(config: Config) -> Result<()> {
    info!("starting mdnsview {}", env!("CARGO_PKG_VERSION"));
    let (query_tx, query_rx) = mpsc::unbounded_channel();
    let (mut store, _store_task) = model::store::spawn(config.device_timeout, Some(query_tx));

    let selector = Selector::new(&config.interfaces)?;
    let wire_error = match WireBackend::open(selector, config.ipv6, config.device_timeout) {
        Ok(backend) => {
            tokio::spawn(backend.run(store.observations.clone(), query_rx));
            None
        }
        Err(e) => {
            warn!("multicast socket unavailable: {e}");
            let reason = format!("cannot bind UDP port {}: {e}", wire::sys::MDNS_PORT);
            store
                .observations
                .send(Observation::SourceStatus {
                    source: Source::Wire,
                    state: SourceState::Unavailable(reason.clone()),
                })
                .await?;
            Some(reason)
        }
    };

    let system = backend::platform();
    let system_name = system.name();
    tokio::spawn(system.run(store.observations.clone()));

    if let Some(wire_error) = wire_error {
        // Without our own socket the daemon is the only source: make sure it works.
        let system_state = tokio::time::timeout(SYSTEM_STARTUP_TIMEOUT, async {
            loop {
                let state = store.snapshots.borrow_and_update().status.system.clone();
                if state != SourceState::Starting {
                    return state;
                }
                if store.snapshots.changed().await.is_err() {
                    return SourceState::Unavailable("store stopped".into());
                }
            }
        })
        .await
        .unwrap_or_else(|_| SourceState::Unavailable(format!("{system_name} did not respond")));
        if let SourceState::Unavailable(system_error) = system_state {
            return Err(anyhow!(
                "no discovery source available:\n  multicast socket: {wire_error}\n  {system_name}: {system_error}"
            ));
        }
    }

    ui::run(store, system_name, config.mouse).await
}
