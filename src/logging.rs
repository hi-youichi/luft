//! Program-log (tracing) initialization — see `docs/design/program-logging.md`.
//!
//! Installs the global `tracing` subscriber so the diagnostic/operational log
//! (spawn failures, retry decisions, protocol errors, …) is actually emitted.
//! This is the **program log** plane — distinct from the event log (`AgentEvent`
//! → `EventLogger`).

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Install the global tracing subscriber. Idempotent — a second call is a no-op
/// (the underlying `try_init` returns `Err` once a global subscriber exists,
/// which we swallow).
///
/// Filter precedence: `level` (`--log-level`) > `RUST_LOG` > `default`.
pub fn init(level: Option<&str>, default: &str) -> anyhow::Result<()> {
    let filter = level
        .and_then(|l| EnvFilter::try_new(l).ok())
        .or_else(|| EnvFilter::try_from_default_env().ok())
        .unwrap_or_else(|| EnvFilter::new(default));

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_target(false))
        .try_init();

    Ok(())
}
