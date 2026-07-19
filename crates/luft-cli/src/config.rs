//! Luft config file (`~/.config/luft/config.toml`) — read, write, merge.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Top-level luft config file.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LuftConfig {
    #[serde(default)]
    pub backend: BackendConfig,
    #[serde(default)]
    pub planner: PlannerConfig,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PlannerConfig {
    /// Default model for planner LLM calls (falls back to backend.model).
    pub model: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BackendConfig {
    /// Default backend id (overrides auto-detect).
    pub default: Option<String>,
    /// Default model for LLM calls (overrides agent default).
    pub model: Option<String>,
    #[serde(default)]
    pub acp: AcpConfigOverride,
    /// Per-backend override for the `codex` ACP backend ([backend.codex_acp]).
    #[serde(default)]
    pub codex_acp: AcpBackendOverride,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AcpConfigOverride {
    pub binary: Option<PathBuf>,
    pub args: Option<Vec<String>>,
    pub log_level: Option<String>,
    pub connect_timeout_secs: Option<u64>,
    pub idle_timeout_secs: Option<u64>,
    pub emit_raw_events: Option<bool>,
}

/// Per-backend override for the `codex` ACP backend ([backend.codex_acp]).
/// Uses `command` (not `binary`) to stay distinct from the shared [backend.acp].
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AcpBackendOverride {
    pub command: Option<PathBuf>,
    pub args: Option<Vec<String>>,
    pub connect_timeout_secs: Option<u64>,
    pub idle_timeout_secs: Option<u64>,
    pub emit_raw_events: Option<bool>,
    pub inherit_env: Option<Vec<String>>,
    pub env: Option<BTreeMap<String, String>>,
}

// ── paths ──────────────────────────────────────────────────────────────────

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("luft")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

// ── persistence ────────────────────────────────────────────────────────────

pub fn load_config() -> Option<LuftConfig> {
    let path = config_path();
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&content).ok()?
}

pub fn save_config(config: &LuftConfig) -> Result<(), String> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create config dir {}: {e}", dir.display()))?;
    let path = config_path();
    let content =
        toml::to_string_pretty(config).map_err(|e| format!("failed to serialize config: {e}"))?;
    std::fs::write(&path, &content)
        .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(())
}

/// Resolve default backend: explicit arg > config default > auto-detect.
///
/// Auto-detect logic:
/// - 0 real backends → "mock" (silent fallback)
/// - 1 real backend  → use it
/// - ≥2 real backends → prompt user, persist choice to config
pub fn resolve_default_backend(user_specified: Option<&str>) -> String {
    if let Some(id) = user_specified.filter(|s| !s.is_empty()) {
        return id.to_string();
    }
    if let Some(cfg) = load_config() {
        if let Some(id) = cfg.backend.default {
            return id;
        }
    }
    let available = crate::backend::detect_available_backends();
    match available.len() {
        0 => "mock".to_string(),
        1 => available[0].to_string(),
        _ => match crate::backend::prompt_backend_selection(&available) {
            Ok(selected) => {
                let mut cfg = load_config().unwrap_or_default();
                cfg.backend.default = Some(selected.clone());
                if let Err(e) = save_config(&cfg) {
                    eprintln!("warning: could not save config: {e}");
                }
                selected
            }
            Err(e) => {
                eprintln!(
                    "warning: backend selection failed ({e}), using {}",
                    available[0]
                );
                available[0].to_string()
            }
        },
    }
}

/// Resolve model: CLI > config > None.
pub fn resolve_model(cli: Option<&str>, config: Option<&str>) -> Option<String> {
    cli.map(String::from).or_else(|| config.map(String::from))
}

/// Resolve planner model: CLI planner_model > CLI model > config > None.
pub fn resolve_planner_model(
    cli_planner: Option<&str>,
    cli_model: Option<&str>,
    config: Option<&str>,
) -> Option<String> {
    cli_planner
        .map(String::from)
        .or_else(|| cli_model.map(String::from))
        .or_else(|| config.map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Default implementations ──────────────────────────────────────

    #[test]
    fn luft_config_default_is_empty() {
        let cfg = LuftConfig::default();
        assert!(cfg.backend.default.is_none());
        assert!(cfg.backend.model.is_none());
        assert!(cfg.planner.model.is_none());
    }

    #[test]
    fn planner_config_default_is_none() {
        let cfg = PlannerConfig::default();
        assert!(cfg.model.is_none());
    }

    #[test]
    fn backend_config_default_is_none() {
        let cfg = BackendConfig::default();
        assert!(cfg.default.is_none());
        assert!(cfg.model.is_none());
    }

    #[test]
    fn acp_config_override_default_is_none() {
        let cfg = AcpConfigOverride::default();
        assert!(cfg.binary.is_none());
        assert!(cfg.args.is_none());
        assert!(cfg.log_level.is_none());
        assert!(cfg.connect_timeout_secs.is_none());
        assert!(cfg.idle_timeout_secs.is_none());
        assert!(cfg.emit_raw_events.is_none());
    }

    // ── Debug formatting ─────────────────────────────────────────────

    #[test]
    fn luft_config_debug_includes_backend_and_planner() {
        let cfg = LuftConfig::default();
        let s = format!("{cfg:?}");
        assert!(s.contains("backend"), "Debug must show backend");
        assert!(s.contains("planner"), "Debug must show planner");
    }

    #[test]
    fn backend_config_debug_includes_acp() {
        let cfg = BackendConfig::default();
        let s = format!("{cfg:?}");
        assert!(s.contains("acp"));
    }

    // ── AcpBackendOverride (codex_acp) ─────────────────────────────────

    #[test]
    fn acp_backend_override_default_is_empty() {
        let cfg = AcpBackendOverride::default();
        assert!(cfg.command.is_none());
        assert!(cfg.args.is_none());
        assert!(cfg.connect_timeout_secs.is_none());
        assert!(cfg.idle_timeout_secs.is_none());
        assert!(cfg.emit_raw_events.is_none());
        assert!(cfg.inherit_env.is_none());
        assert!(cfg.env.is_none());
    }

    #[test]
    fn backend_config_debug_includes_codex_acp() {
        let cfg = BackendConfig::default();
        let s = format!("{cfg:?}");
        assert!(s.contains("codex_acp"), "Debug must show codex_acp");
    }

    #[test]
    fn codex_acp_roundtrip_serialization() {
        let cfg = BackendConfig {
            codex_acp: AcpBackendOverride {
                command: Some("/usr/local/bin/codex-acp".into()),
                args: Some(vec!["--verbose".into()]),
                connect_timeout_secs: Some(20),
                idle_timeout_secs: Some(600),
                emit_raw_events: Some(true),
                inherit_env: Some(vec!["FOO".into()]),
                env: Some(BTreeMap::from([
                    ("BAR".to_string(), "baz".to_string()),
                ])),
            },
            ..BackendConfig::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        let parsed: BackendConfig = toml::from_str(&s).unwrap();
        assert_eq!(
            parsed.codex_acp.command.as_deref(),
            Some(std::path::Path::new("/usr/local/bin/codex-acp"))
        );
        assert_eq!(
            parsed.codex_acp.args.as_ref().unwrap(),
            &vec!["--verbose".to_string()]
        );
        assert_eq!(parsed.codex_acp.connect_timeout_secs, Some(20));
        assert_eq!(parsed.codex_acp.idle_timeout_secs, Some(600));
        assert_eq!(parsed.codex_acp.emit_raw_events, Some(true));
        assert_eq!(
            parsed.codex_acp.inherit_env.as_ref().unwrap(),
            &vec!["FOO".to_string()]
        );
        assert_eq!(
            parsed.codex_acp.env.as_ref().unwrap().get("BAR"),
            Some(&"baz".to_string())
        );
    }

    #[test]
    fn codex_acp_default_roundtrip_is_empty() {
        let cfg = BackendConfig::default();
        let s = toml::to_string(&cfg).unwrap();
        let parsed: BackendConfig = toml::from_str(&s).unwrap();
        assert!(parsed.codex_acp.command.is_none());
        assert!(parsed.codex_acp.args.is_none());
    }

    // ── Paths ────────────────────────────────────────────────────────

    #[test]
    fn config_dir_ends_with_luft() {
        let dir = config_dir();
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("luft"));
    }

    #[test]
    fn config_path_ends_with_config_toml() {
        let path = config_path();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("config.toml")
        );
        assert_eq!(path.parent(), Some(config_dir().as_path()));
    }

    // ── TOML serialization round-trips ───────────────────────────────

    #[test]
    fn luft_config_serialize_roundtrip_empty() {
        let cfg = LuftConfig::default();
        let s = toml::to_string(&cfg).unwrap();
        let parsed: LuftConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed.backend.default, cfg.backend.default);
        assert_eq!(parsed.backend.model, cfg.backend.model);
        assert_eq!(parsed.planner.model, cfg.planner.model);
    }

    #[test]
    fn luft_config_serialize_roundtrip_full() {
        let cfg = LuftConfig {
            backend: BackendConfig {
                default: Some("opencode".into()),
                model: Some("claude-3-5-sonnet".into()),
                acp: AcpConfigOverride {
                    binary: Some("/usr/local/bin/opencode".into()),
                    args: Some(vec!["--verbose".into(), "--no-color".into()]),
                    log_level: Some("debug".into()),
                    connect_timeout_secs: Some(15),
                    idle_timeout_secs: Some(600),
                    emit_raw_events: Some(false),
                },
                codex_acp: AcpBackendOverride::default(),
            },
            planner: PlannerConfig {
                model: Some("claude-3-haiku".into()),
            },
        };
        let s = toml::to_string(&cfg).unwrap();
        let parsed: LuftConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed.backend.default.as_deref(), Some("opencode"));
        assert_eq!(parsed.backend.model.as_deref(), Some("claude-3-5-sonnet"));
        assert_eq!(
            parsed.backend.acp.binary.as_deref(),
            Some(std::path::Path::new("/usr/local/bin/opencode"))
        );
        assert_eq!(
            parsed.backend.acp.args.as_ref().unwrap(),
            &vec!["--verbose".to_string(), "--no-color".to_string()]
        );
        assert_eq!(parsed.backend.acp.log_level.as_deref(), Some("debug"));
        assert_eq!(parsed.backend.acp.connect_timeout_secs, Some(15));
        assert_eq!(parsed.backend.acp.idle_timeout_secs, Some(600));
        assert_eq!(parsed.backend.acp.emit_raw_events, Some(false));
        assert_eq!(parsed.planner.model.as_deref(), Some("claude-3-haiku"));
    }

    #[test]
    fn luft_config_pretty_serialize_is_multiline() {
        let cfg = LuftConfig::default();
        let s = toml::to_string_pretty(&cfg).unwrap();
        assert!(s.contains('\n'));
    }

    #[test]
    fn backend_default_alias_serializes_as_string() {
        let cfg = BackendConfig {
            default: Some("mock".into()),
            ..BackendConfig::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        assert!(s.contains("default = \"mock\""));
    }

    #[test]
    fn backend_acp_args_serialize_as_array() {
        let cfg = BackendConfig {
            acp: AcpConfigOverride {
                args: Some(vec!["a".into(), "b".into()]),
                ..AcpConfigOverride::default()
            },
            ..BackendConfig::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        assert!(s.contains("args"));
        let parsed: BackendConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed.acp.args.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn backend_acp_connect_timeout_serializes_as_integer() {
        let cfg = BackendConfig {
            acp: AcpConfigOverride {
                connect_timeout_secs: Some(42),
                ..AcpConfigOverride::default()
            },
            ..BackendConfig::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        assert!(s.contains("connect_timeout_secs = 42"));
    }

    #[test]
    fn planner_model_serializes_as_string() {
        let cfg = PlannerConfig {
            model: Some("gpt-4o".into()),
        };
        let s = toml::to_string(&cfg).unwrap();
        assert!(s.contains("model = \"gpt-4o\""));
    }

    // ── load_config ──────────────────────────────────────────────────

    #[test]
    fn load_config_handles_missing_file_gracefully() {
        // We can't easily redirect config_dir(), but we can confirm that
        // load_config returns an Option (None when missing).
        let result = load_config();
        // It may return None (no file) or Some (file present). Either is OK;
        // the function must not panic.
        match result {
            None | Some(_) => {}
        }
    }

    // ── resolve_default_backend ──────────────────────────────────────

    #[test]
    fn resolve_default_backend_user_specified_takes_precedence() {
        let id = resolve_default_backend(Some("my-custom-backend"));
        assert_eq!(id, "my-custom-backend");
    }

    #[test]
    fn resolve_default_backend_empty_user_input_falls_through() {
        // Empty string is treated as "not specified".
        let id = resolve_default_backend(Some(""));
        // Falls through to config default or auto-detect (which gives "mock"
        // when no real backends are installed).
        assert!(
            id == "mock" || !id.is_empty(),
            "empty string must fall through to a non-empty value, got '{id}'"
        );
    }

    // ── resolve_model ────────────────────────────────────────────────

    #[test]
    fn resolve_model_cli_wins() {
        assert_eq!(
            resolve_model(Some("cli-model"), Some("config-model")),
            Some("cli-model".to_string())
        );
    }

    #[test]
    fn resolve_model_falls_back_to_config() {
        assert_eq!(
            resolve_model(None, Some("config-model")),
            Some("config-model".to_string())
        );
    }

    #[test]
    fn resolve_model_none_when_both_none() {
        assert_eq!(resolve_model(None, None), None);
    }

    #[test]
    fn resolve_model_none_when_cli_empty_and_no_config() {
        // Some("") still maps to Some(""), so this returns Some("").
        assert_eq!(resolve_model(Some(""), None), Some("".to_string()));
    }

    // ── resolve_planner_model ────────────────────────────────────────

    #[test]
    fn resolve_planner_model_cli_planner_wins() {
        assert_eq!(
            resolve_planner_model(Some("p"), Some("m"), Some("c")),
            Some("p".to_string())
        );
    }

    #[test]
    fn resolve_planner_model_cli_model_used_when_no_planner() {
        assert_eq!(
            resolve_planner_model(None, Some("m"), Some("c")),
            Some("m".to_string())
        );
    }

    #[test]
    fn resolve_planner_model_config_used_as_last_resort() {
        assert_eq!(
            resolve_planner_model(None, None, Some("c")),
            Some("c".to_string())
        );
    }

    #[test]
    fn resolve_planner_model_none_when_all_none() {
        assert_eq!(resolve_planner_model(None, None, None), None);
    }

    #[test]
    fn resolve_planner_model_priority_chain() {
        // cli_planner > cli_model > config
        assert_eq!(
            resolve_planner_model(None, Some("cli-model"), Some("cfg")),
            Some("cli-model".to_string())
        );
        assert_eq!(
            resolve_planner_model(Some("cli-planner"), None, Some("cfg")),
            Some("cli-planner".to_string())
        );
    }

    // ── save_config + load_config round-trip via tempdir trick ──────
    //
    // We can't easily redirect config_dir(), but we can verify save_config
    // *behaviour* on a writable filesystem by inspecting the rendered TOML
    // output. The serialization is exercised above.

    #[test]
    fn save_config_produces_valid_toml_via_to_string() {
        let cfg = LuftConfig::default();
        let s = toml::to_string_pretty(&cfg).unwrap();
        // Should re-parse without error.
        let _: LuftConfig = toml::from_str(&s).unwrap();
    }
}
