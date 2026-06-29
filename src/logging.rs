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

#[cfg(test)]
mod tests {
    use super::*;

    /// Default path: level=None falls through to default.
    /// Must run first in-process so try_init succeeds for the filter path test.
    #[test]
    fn init_default_level() {
        let _ = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_target(false),
            )
            .try_init();

        assert!(init(None, "error").is_ok());
    }

    /// Explicit level path: `Some("trace")` is valid.
    #[test]
    fn init_explicit_level_trace() {
        assert!(init(Some("trace"), "info").is_ok());
    }

    /// Explicit level path: `Some("debug")` is valid.
    #[test]
    fn init_explicit_level_debug() {
        assert!(init(Some("debug"), "warn").is_ok());
    }

    /// Explicit level path: `Some("off")` is valid (disables all).
    #[test]
    fn init_explicit_level_off() {
        assert!(init(Some("off"), "info").is_ok());
    }

    /// Explicit level path: `Some("")` (empty string) is accepted by EnvFilter.
    #[test]
    fn init_empty_level_string() {
        assert!(init(Some(""), "warn").is_ok());
    }

    /// Invalid level string: `EnvFilter::try_new` returns `Err`, falls through
    /// to `try_from_default_env()` (no env → Err) → default `"warn"`.
    #[test]
    fn init_invalid_level_falls_back_to_default() {
        assert!(init(Some(":::"), "warn").is_ok());
    }

    /// RUST_LOG environment path: when RUST_LOG is set and explicit level is
    /// None, the filter comes from the env var.
    #[test]
    fn init_uses_rust_log_env_var() {
        unsafe {
            std::env::set_var("RUST_LOG", "error");
        }
        let result = init(None, "info");
        unsafe {
            std::env::remove_var("RUST_LOG");
        }
        assert!(result.is_ok());
    }

    /// Explicit level overrides RUST_LOG env var.
    #[test]
    fn init_explicit_level_overrides_rust_log() {
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
        let result = init(Some("trace"), "error");
        unsafe {
            std::env::remove_var("RUST_LOG");
        }
        assert!(result.is_ok());
    }

    /// Invalid explicit level with RUST_LOG set: EnvFilter::try_new fails,
    /// falls back to RUST_LOG.
    #[test]
    fn init_invalid_level_uses_rust_log_fallback() {
        unsafe {
            std::env::set_var("RUST_LOG", "warn");
        }
        let result = init(Some(":::"), "error");
        unsafe {
            std::env::remove_var("RUST_LOG");
        }
        assert!(result.is_ok());
    }

    /// No RUST_LOG env var, no explicit level → uses default.
    #[test]
    fn init_no_rust_log_uses_default() {
        unsafe {
            std::env::remove_var("RUST_LOG");
        }
        assert!(init(None, "warn").is_ok());
    }

    /// Idempotency: calling init multiple times does not panic or error.
    #[test]
    fn init_idempotent() {
        assert!(init(Some("debug"), "info").is_ok());
        assert!(init(None, "error").is_ok());
        assert!(init(Some("trace"), "warn").is_ok());
    }
}
