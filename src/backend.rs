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
