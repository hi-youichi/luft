//! Program-log (tracing) initialization — see `docs/design/program-logging.md`.
//!
//! Installs the global `tracing` subscriber so the diagnostic/operational log
//! (spawn failures, retry decisions, protocol errors, …) is actually emitted.
//! This is the **program log** plane — distinct from the event log (`AgentEvent`
//! → `EventLogger`).

use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Install the global tracing subscriber. Idempotent — a second call is a no-op
/// (the underlying `try_init` returns `Err` once a global subscriber exists,
/// which we swallow).
///
/// Filter precedence: `level` (`--log-level`) > `RUST_LOG` > `default`
/// (per-subcommand, e.g. `serve`=`info`, `run`=`warn`).
///
/// When `file` is given, logs are additionally written there via a non-blocking
/// appender. The returned [`WorkerGuard`] must be kept alive for the process
/// lifetime — dropping it flushes and stops the writer thread.
pub fn init(
    level: Option<&str>,
    default: &str,
    file: Option<&Path>,
) -> anyhow::Result<Option<WorkerGuard>> {
    let filter = level
        .and_then(|l| EnvFilter::try_new(l).ok())
        .or_else(|| EnvFilter::try_from_default_env().ok())
        .unwrap_or_else(|| EnvFilter::new(default));

    let stderr_layer = fmt::layer().with_writer(std::io::stderr).with_target(false);

    let (file_layer, guard) = match file {
        Some(path) => {
            let f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
            let (nb, guard) = tracing_appender::non_blocking(f);
            let layer = fmt::layer().with_writer(nb).with_ansi(false).with_target(false);
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    // `Option<Layer>` is itself a no-op `Layer` when `None`.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init();

    Ok(guard)
}
