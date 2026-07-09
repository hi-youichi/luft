//! Backend factory: construct an [`AgentBackend`] by id, with auto-detection.

use anyhow::Result;
use maestro::core::{AgentBackend, MockBackend, MockBehavior, TokenUsage};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Construct a backend by id. `emit_raw_events` toggles the ACP backend's raw
/// `session/update` passthrough (ignored by the mock backend).
/// `model` sets the LLM model for ACP backends (passed via ACP config options).
pub fn create_backend(
    id: &str,
    emit_raw_events: bool,
    model: Option<String>,
) -> Result<Arc<dyn AgentBackend>> {
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
            apply_acp_overrides(maestro::adapters::AcpConfig {
                emit_raw_events,
                model: model.clone(),
                ..Default::default()
            }),
        ))),
        "loom-acp" => Ok(Arc::new(maestro::adapters::AcpAdapter::new(
            apply_acp_overrides(maestro::adapters::AcpConfig {
                id: "loom-acp",
                binary: PathBuf::from("loom-acp"),
                acp_args: vec![],
                emit_raw_events,
                model,
                ..Default::default()
            }),
        ))),
        _ => anyhow::bail!("unknown backend: {}", id),
    }
}

/// Merge config file ACP overrides into the given config.
fn apply_acp_overrides(mut cfg: maestro::adapters::AcpConfig) -> maestro::adapters::AcpConfig {
    let over = crate::config::load_config()
        .map(|c| c.backend.acp)
        .unwrap_or_default();
    if let Some(b) = over.binary {
        cfg.binary = b;
    }
    if let Some(v) = over.log_level {
        cfg.log_level = Some(v);
    }
    if let Some(s) = over.connect_timeout_secs {
        cfg.connect_timeout = std::time::Duration::from_secs(s);
    }
    if let Some(v) = over.emit_raw_events {
        cfg.emit_raw_events = v;
    }
    if let Some(a) = over.args {
        cfg.acp_args = a;
    }
    cfg
}

pub fn detect_backend() -> &'static str {
    if which_exists("opencode") {
        "opencode"
    } else {
        "mock"
    }
}

/// Detect all available real LLM backends (excludes mock).
/// Checks PATH and config override for each known backend binary.
pub fn detect_available_backends() -> Vec<&'static str> {
    let cfg = crate::config::load_config();
    let override_binary = cfg.as_ref().and_then(|c| c.backend.acp.binary.as_deref());

    ["opencode", "loom-acp"]
        .into_iter()
        .filter(|id| is_binary_available(id, override_binary))
        .collect()
}

/// Check whether a backend's binary is available on PATH or at an absolute
/// config-overridden path.
fn is_binary_available(id: &str, override_binary: Option<&Path>) -> bool {
    let binary = override_binary
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(id));
    if binary.is_absolute() {
        binary.exists()
    } else {
        which_exists(binary.to_str().unwrap_or(id))
    }
}

/// Prompt the user to select from multiple available backends.
/// Non-interactive (piped stdin/stdout) auto-selects the first option.
pub fn prompt_backend_selection(backends: &[&str]) -> Result<String> {
    if !console::user_attended() {
        return Ok(backends[0].to_string());
    }

    eprintln!("Multiple backends available. Select one:");
    for (i, id) in backends.iter().enumerate() {
        eprintln!("  [{}] {}", i + 1, id);
    }
    eprint!("Enter number (default 1): ");
    io::stderr().flush().ok();

    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;

    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(backends[0].to_string());
    }
    let idx: usize = trimmed
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid selection: '{trimmed}', expected a number"))?;
    if idx == 0 || idx > backends.len() {
        anyhow::bail!("selection {idx} out of range (1-{})", backends.len());
    }
    Ok(backends[idx - 1].to_string())
}

pub(crate) fn which_exists(cmd: &str) -> bool {
    let checker = if cfg!(windows) { "where" } else { "which" };
    std::process::Command::new(checker)
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
        let backend = create_backend("mock", false, None).unwrap();
        assert_eq!(backend.id(), "mock");
    }

    #[test]
    fn create_backend_returns_opencode() {
        let backend = create_backend("opencode", false, None).unwrap();
        assert_eq!(backend.id(), "opencode");
    }

    #[test]
    fn create_backend_emit_raw_events_true() {
        let backend = create_backend("opencode", true, None).unwrap();
        assert_eq!(backend.id(), "opencode");
    }

    #[test]
    fn create_backend_with_model() {
        let backend = create_backend("opencode", false, Some("claude-3".into())).unwrap();
        assert_eq!(backend.id(), "opencode");
    }

    #[test]
    fn create_backend_unknown_id() {
        match create_backend("bogus", false, None) {
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

    // ── detect_available_backends ──────────────────────────────

    #[test]
    fn detect_available_backends_returns_subset() {
        let backends = detect_available_backends();
        for id in &backends {
            assert!(
                *id == "opencode" || *id == "loom-acp",
                "unexpected backend id: {id}",
            );
        }
    }

    // ── is_binary_available ─────────────────────────────────────

    #[test]
    #[serial_test::serial]
    fn is_binary_available_path_command() {
        let cmd = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(is_binary_available(cmd, None));
    }

    #[test]
    fn is_binary_available_missing_command() {
        assert!(!is_binary_available("nonexistent_xyzzy_42", None));
    }

    #[test]
    #[serial_test::serial]
    fn is_binary_available_absolute_override_existing() {
        let cmd = if cfg!(windows) { "cmd" } else { "sh" };
        let path = std::process::Command::new(if cfg!(windows) { "where" } else { "which" })
            .arg(cmd)
            .output()
            .unwrap();
        let resolved = String::from_utf8_lossy(&path.stdout);
        let first = resolved.lines().next().unwrap_or(cmd);
        let abs = std::path::PathBuf::from(first);
        assert!(is_binary_available(cmd, Some(&abs)));
    }

    #[test]
    fn is_binary_available_absolute_override_missing() {
        let abs = std::path::PathBuf::from("/__nonexistent__/binary_42");
        assert!(!is_binary_available("opencode", Some(&abs)));
    }

    // ── prompt_backend_selection ────────────────────────────────

    #[test]
    fn prompt_backend_selection_single() {
        let result = prompt_backend_selection(&["opencode"]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "opencode");
    }

    // ── which_exists ────────────────────────────────────────────

    #[test]
    #[serial_test::serial]
    fn which_exists_finds_existing_command() {
        // On Unix, `echo` is typically at /bin/echo or /usr/bin/echo.
        // On Windows, `echo` is a cmd builtin, not a standalone executable.
        // Use `cmd` (Windows) or `sh` (Unix) as the known-present binary.
        let cmd = if cfg!(windows) { "cmd" } else { "echo" };
        assert!(which_exists(cmd));
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
