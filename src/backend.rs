//! Backend factory: construct an [`AgentBackend`] by id, with auto-detection.

use anyhow::Result;
use maestro::core::{AgentBackend, MockBackend, MockBehavior, TokenUsage};
use std::sync::Arc;

/// Construct a backend by id. `emit_raw_events` toggles the ACP backend's raw
/// `session/update` passthrough (ignored by the mock backend).
pub fn create_backend(id: &str, emit_raw_events: bool) -> Result<Arc<dyn AgentBackend>> {
    match id {
        "mock" => Ok(Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::Value::Null,
                tokens: TokenUsage::default(),
                delay: std::time::Duration::from_millis(10),
            }],
        ))),
        "opencode" => Ok(Arc::new(maestro::adapters::AcpAdapter::new(
            maestro::adapters::AcpConfig {
                emit_raw_events,
                ..Default::default()
            },
        ))),
        _ => anyhow::bail!("unknown backend: {}", id),
    }
}

pub fn detect_backend() -> &'static str {
    if which_exists("opencode") {
        "opencode"
    } else {
        "mock"
    }
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── create_backend ──────────────────────────────────────────

    #[test]
    fn create_backend_returns_mock() {
        let backend = create_backend("mock", false).unwrap();
        assert_eq!(backend.id(), "mock");
    }

    #[test]
    fn create_backend_returns_opencode() {
        let backend = create_backend("opencode", false).unwrap();
        assert_eq!(backend.id(), "opencode");
    }

    #[test]
    fn create_backend_emit_raw_events_true() {
        let backend = create_backend("opencode", true).unwrap();
        assert_eq!(backend.id(), "opencode");
    }

    #[test]
    fn create_backend_unknown_id() {
        match create_backend("bogus", false) {
            Err(e) => assert!(e.to_string().contains("unknown backend")),
            Ok(_) => panic!("expected error for unknown backend"),
        }
    }

    // ── detect_backend ──────────────────────────────────────────

    #[test]
    fn detect_backend_returns_valid_id() {
        let id = detect_backend();
        assert!(
            id == "opencode" || id == "mock",
            "unexpected backend id: {id}",
        );
    }

    // ── which_exists ────────────────────────────────────────────

    #[test]
    fn which_exists_finds_existing_command() {
        assert!(which_exists("echo"));
    }

    #[test]
    fn which_exists_returns_false_for_missing_command() {
        assert!(!which_exists("nonexistent_cmd_xyzzy_42"));
    }

    /// `which` itself may fail in pathological environments; the
    /// `unwrap_or(false)` arm handles that case. At minimum the
    /// function should never panic.
    #[test]
    fn which_exists_never_panics() {
        let _ = which_exists("echo");
        let _ = which_exists("");
    }
}
