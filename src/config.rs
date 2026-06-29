//! Maestro config file (`~/.config/maestro/config.toml`) — read, write, merge.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level maestro config file.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MaestroConfig {
    #[serde(default)]
    pub backend: BackendConfig,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BackendConfig {
    /// Default backend id (overrides auto-detect).
    pub default: Option<String>,
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
        .join("maestro")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

// ── persistence ────────────────────────────────────────────────────────────

pub fn load_config() -> Option<MaestroConfig> {
    let path = config_path();
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&content).ok()?
}

pub fn save_config(config: &MaestroConfig) -> Result<(), String> {
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
pub fn resolve_default_backend(user_specified: Option<&str>) -> String {
    if let Some(id) = user_specified.filter(|s| !s.is_empty()) {
        return id.to_string();
    }
    if let Some(cfg) = load_config() {
        if let Some(id) = cfg.backend.default {
            return id;
        }
    }
    crate::backend::detect_backend().to_string()
}
