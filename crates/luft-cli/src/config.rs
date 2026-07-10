//! Luft config file (`~/.config/luft/config.toml`) — read, write, merge.

use serde::{Deserialize, Serialize};
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
