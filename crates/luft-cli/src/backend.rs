//! Backend factory: construct an [`AgentBackend`] by id, with auto-detection.

use anyhow::Result;
use luft::core::{AgentBackend, MockBackend, MockBehavior, TokenUsage};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Maps a backend id to the default executable name resolved from `PATH`.
/// Most ids double as their own binary name; `claude-acp` is the exception —
/// it wraps the official `claude-code-acp` npm package (see
/// `docs/architecture/adapters.md`).
pub(crate) fn default_binary_name(id: &str) -> String {
    match id {
        "claude-acp" => "claude-code-acp".to_string(),
        other => other.to_string(),
    }
}

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
        "opencode" => Ok(Arc::new(luft::adapters::AcpAdapter::new(
            apply_acp_overrides(luft::adapters::AcpConfig {
                emit_raw_events,
                model: model.clone(),
                ..Default::default()
            }),
        ))),
        "loom-acp" => Ok(Arc::new(luft::adapters::AcpAdapter::new(
            apply_acp_overrides(luft::adapters::AcpConfig {
                id: "loom-acp",
                binary: PathBuf::from("loom-acp"),
                acp_args: vec![],
                emit_raw_events,
                model,
                ..Default::default()
            }),
        ))),
        "claude-acp" => Ok(Arc::new(luft::adapters::AcpAdapter::new(
            apply_acp_overrides(luft::adapters::AcpConfig {
                id: "claude-acp",
                binary: PathBuf::from(default_binary_name("claude-acp")),
                acp_args: vec![],
                emit_raw_events,
                model,
                // `claude-code-acp` (the official Claude Agent SDK ACP wrapper)
                // authenticates via `ANTHROPIC_API_KEY`. Its terminal-based
                // OAuth login flows require a real TTY, which the ACP
                // subprocess never gets (stdin/stdout are piped for the
                // JSON-RPC transport, stderr is `/dev/null`) — so the API key
                // is the only auth path that works headless. This is a
                // deliberate, backend-scoped exception to the "no provider
                // keys by default" policy documented on
                // `AcpConfig::env_passthrough`.
                env_passthrough: {
                    let mut v: Vec<String> = luft::adapters::AcpConfig::DEFAULT_ENV_PASSTHROUGH
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    v.push("ANTHROPIC_API_KEY".to_string());
                    v
                },
                ..Default::default()
            }),
        ))),
        "codex" => {
            let binary = if cfg!(windows) {
                PathBuf::from("npx.cmd")
            } else {
                PathBuf::from("npx")
            };
            let cfg = apply_codex_acp_overrides(luft::adapters::AcpConfig {
                id: "codex",
                binary,
                acp_args: vec![
                    "-y".to_string(),
                    "@agentclientprotocol/codex-acp".to_string(),
                ],
                log_level: None,
                emit_raw_events,
                model,
                ..Default::default()
            });
            Ok(Arc::new(luft::adapters::AcpAdapter::new(cfg)))
        }
        _ => anyhow::bail!("unknown backend: {}", id),
    }
}

/// Merge the dedicated Codex ACP configuration without allowing secrets to be
/// stored in the TOML file. Credential variables must use `inherit_env`.
fn apply_codex_acp_overrides(mut cfg: luft::adapters::AcpConfig) -> luft::adapters::AcpConfig {
    let over = crate::config::load_config()
        .map(|c| c.backend.codex_acp)
        .unwrap_or_default();
    if let Some(command) = over.command {
        cfg.binary = command;
    }
    if let Some(args) = over.args {
        cfg.acp_args = args;
    }
    if let Some(timeout) = over.connect_timeout_secs {
        cfg.connect_timeout = Duration::from_secs(timeout);
    }
    if let Some(emit_raw_events) = over.emit_raw_events {
        cfg.emit_raw_events = emit_raw_events;
    }
    if let Some(names) = over.inherit_env {
        for name in names {
            if !cfg.env_passthrough.contains(&name) {
                cfg.env_passthrough.push(name);
            }
        }
    }
    if let Some(env) = over.env {
        for (name, value) in env {
            if is_sensitive_env_var(&name) {
                tracing::warn!(env = %name, "ignoring sensitive Codex ACP env configured in file; use inherit_env instead");
            } else {
                cfg.env.insert(name, value);
            }
        }
    }
    cfg.log_level = None; // codex-acp does not accept --log-level
    cfg
}

fn is_sensitive_env_var(name: &str) -> bool {
    let normalized = name.to_ascii_uppercase();
    normalized.contains("KEY")
        || normalized.contains("TOKEN")
        || normalized.contains("SECRET")
        || normalized.contains("PASSWORD")
}

/// Merge config file ACP overrides into the given config.
fn apply_acp_overrides(mut cfg: luft::adapters::AcpConfig) -> luft::adapters::AcpConfig {
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

/// Auto-detect priority: `opencode` first (multi-provider, most commonly
/// installed), then `claude-acp` (official Claude Agent SDK wrapper, single
/// provider), falling back to `mock` when neither binary is on `PATH`.
pub fn detect_backend() -> &'static str {
    if which_exists("opencode") {
        "opencode"
    } else if which_exists(&default_binary_name("claude-acp")) {
        "claude-acp"
    } else {
        "mock"
    }
}

/// Detect all available real LLM backends (excludes mock).
/// Checks PATH and config override for each known backend binary.
pub fn detect_available_backends() -> Vec<&'static str> {
    let cfg = crate::config::load_config();
    let override_binary = cfg.as_ref().and_then(|c| c.backend.acp.binary.as_deref());

    let mut result: Vec<&'static str> = ["opencode", "claude-acp", "loom-acp"]
        .into_iter()
        .filter(|id| is_binary_available(id, override_binary))
        .collect();

    // Probe for codex (npx-based). Check user-configured command first,
    // then the platform-default npx / npx.cmd.
    let codex_cmd = cfg
        .as_ref()
        .and_then(|c| c.backend.codex_acp.command.as_deref());
    let codex_available = if let Some(cmd) = codex_cmd {
        if cmd.is_absolute() {
            cmd.exists()
        } else {
            which_exists(cmd.to_str().unwrap_or("npx"))
        }
    } else if cfg!(windows) {
        which_exists("npx.cmd")
    } else {
        which_exists("npx")
    };
    if codex_available {
        result.push("codex");
    }

    result
}

/// Check whether a backend's binary is available on PATH or at an absolute
/// config-overridden path.
fn is_binary_available(id: &str, override_binary: Option<&Path>) -> bool {
    let binary = override_binary
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default_binary_name(id)));
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
    fn create_backend_returns_claude_acp() {
        let backend = create_backend("claude-acp", false, None).unwrap();
        assert_eq!(backend.id(), "claude-acp");
    }

    #[test]
    fn create_backend_claude_acp_with_model() {
        let backend = create_backend("claude-acp", false, Some("claude-opus-4-8".into())).unwrap();
        assert_eq!(backend.id(), "claude-acp");
    }

    #[test]
    fn create_backend_unknown_id() {
        match create_backend("bogus", false, None) {
            Err(e) => assert!(e.to_string().contains("unknown backend")),
            Ok(_) => panic!("expected error for unknown backend"),
        }
    }

    // ── default_binary_name ──────────────────────────────────────

    #[test]
    fn default_binary_name_maps_claude_acp_to_npm_package_binary() {
        assert_eq!(default_binary_name("claude-acp"), "claude-code-acp");
    }

    #[test]
    fn default_binary_name_identity_for_self_named_backends() {
        assert_eq!(default_binary_name("opencode"), "opencode");
        assert_eq!(default_binary_name("loom-acp"), "loom-acp");
    }

    // ── detect_backend ──────────────────────────────────────────

    #[test]
    fn detect_backend_returns_valid_id() {
        let id = detect_backend();
        assert!(
            id == "opencode" || id == "claude-acp" || id == "mock",
            "unexpected backend id: {id}",
        );
    }

    // ── detect_available_backends ──────────────────────────────

    #[test]
    fn detect_available_backends_returns_subset() {
        let backends = detect_available_backends();
        for id in &backends {
            assert!(
                *id == "opencode"
                    || *id == "claude-acp"
                    || *id == "loom-acp"
                    || *id == "codex",
                "unexpected backend id: {id}",
            );
        }
    }

    #[test]
    fn sensitive_env_names_are_rejected_from_file_config() {
        for name in ["CODEX_API_KEY", "OPENAI_TOKEN", "MY_SECRET", "PASSWORD"] {
            assert!(is_sensitive_env_var(name), "{name} must be sensitive");
        }
        assert!(!is_sensitive_env_var("NO_BROWSER"));
        assert!(!is_sensitive_env_var("INITIAL_AGENT_MODE"));
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

    // ── codex backend ───────────────────────────────────────────

    #[test]
    fn create_backend_codex_returns_codex_id() {
        let backend = create_backend("codex", false, None).unwrap();
        assert_eq!(backend.id(), "codex");
    }

    #[test]
    fn create_backend_codex_with_model() {
        let backend =
            create_backend("codex", false, Some("o4-mini".into())).unwrap();
        assert_eq!(backend.id(), "codex");
    }

    #[test]
    fn create_backend_codex_emit_raw_events() {
        let backend = create_backend("codex", true, None).unwrap();
        assert_eq!(backend.id(), "codex");
    }
}
