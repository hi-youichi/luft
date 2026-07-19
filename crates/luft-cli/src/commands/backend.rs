//! `luft backend` subcommand — list, inspect, check, configure backends.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use agent_client_protocol::schema::{InitializeRequest, ProtocolVersion};
use agent_client_protocol::{ByteStreams, Client};
use clap::Subcommand;
use serde::Serialize;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::config::{config_path, load_config, save_config, LuftConfig};

#[derive(Debug, Subcommand)]
pub enum BackendSubcommand {
    /// List all available backends.
    List,
    /// Show detailed info for a backend.
    Info {
        /// Backend id (default: auto-detected).
        id: Option<String>,
    },
    /// Check if a backend is usable.
    Check {
        /// Backend id (default: auto-detected).
        id: Option<String>,
    },
    /// View or update backend config.
    Config {
        /// Config key path (e.g. `default`, `acp.log_level`).
        key: Option<String>,
        /// Value to set.
        value: Option<String>,
    },
    /// Set the default backend (shorthand for `config default <id>`).
    Set {
        /// Backend id.
        id: String,
    },
}

#[derive(Serialize)]
struct BackendInfo {
    id: String,
    capabilities: CapabilitiesView,
    binary: String,
    config: ConfigView,
}

#[derive(Serialize)]
struct ConfigView {
    args: Vec<String>,
    log_level: Option<String>,
    connect_timeout_secs: u64,
    idle_timeout_secs: u64,
    emit_raw_events: bool,
}

#[derive(Serialize)]
struct CapabilitiesView {
    streaming: bool,
    mcp_injection: bool,
    structured_output: bool,
    models: Vec<String>,
}

pub fn list_backends() {
    let known_ids = &["mock", "loom-acp", "opencode", "claude-acp", "codex"];

    println!(
        "     id     \u{2502} streaming \u{2502} mcp_injection \u{2502} structured_output \u{2502} models"
    );
    println!(
        "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}"
    );

    for id in known_ids {
        match crate::backend::create_backend(id, false, None) {
            Ok(be) => {
                let caps = be.capabilities();
                let models = if caps.models.is_empty() {
                    if *id == "opencode"
                        || *id == "loom-acp"
                        || *id == "claude-acp"
                        || *id == "codex"
                    {
                        "(any)".into()
                    } else {
                        "(n/a)".into()
                    }
                } else {
                    caps.models.join(",")
                };
                println!(
                    "  {:<9}\u{2502}       {}   \u{2502}           {}   \u{2502}                {}  \u{2502} {}",
                    id,
                    bool_mark(caps.streaming),
                    bool_mark(caps.mcp_injection),
                    bool_mark(caps.structured_output),
                    models,
                );
            }
            Err(e) => eprintln!("error creating backend '{id}': {e}"),
        }
    }
}

fn bool_mark(v: bool) -> &'static str {
    if v {
        "\u{2713}"
    } else {
        "\u{2717}"
    }
}

pub fn info_backend(id: Option<String>) {
    let be_id = id.unwrap_or_else(|| crate::backend::detect_backend().to_string());
    let cfg = crate::config::load_config();
    match crate::backend::create_backend(&be_id, false, None) {
        Ok(be) => {
            let caps = be.capabilities();
            let (binary, config) = if be_id == "codex" {
                let codex_cfg = cfg.as_ref().map(|c| &c.backend.codex_acp);
                let bin = codex_cfg
                    .and_then(|c| c.command.as_ref())
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| {
                        if cfg!(windows) {
                            "npx.cmd".into()
                        } else {
                            "npx".into()
                        }
                    });
                let config = ConfigView {
                    args: codex_cfg
                        .and_then(|c| c.args.clone())
                        .unwrap_or_else(codex_default_args),
                    log_level: None, // codex-acp does not accept --log-level
                    connect_timeout_secs: codex_cfg
                        .and_then(|c| c.connect_timeout_secs)
                        .unwrap_or(10),
                    idle_timeout_secs: codex_cfg
                        .and_then(|c| c.idle_timeout_secs)
                        .unwrap_or(300),
                    emit_raw_events: codex_cfg
                        .and_then(|c| c.emit_raw_events)
                        .unwrap_or(true),
                };
                (bin, config)
            } else {
                let bin = match be_id.as_str() {
                    "mock" => "(built-in)".into(),
                    _ => cfg
                        .as_ref()
                        .and_then(|c| c.backend.acp.binary.as_ref())
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| crate::backend::default_binary_name(&be_id).to_string()),
                };
                let acp_cfg = cfg.as_ref().map(|c| &c.backend.acp);
                let config = ConfigView {
                    args: acp_cfg.and_then(|c| c.args.clone()).unwrap_or_default(),
                    log_level: acp_cfg.and_then(|c| c.log_level.clone()),
                    connect_timeout_secs: acp_cfg
                        .and_then(|c| c.connect_timeout_secs)
                        .unwrap_or(10),
                    idle_timeout_secs: acp_cfg.and_then(|c| c.idle_timeout_secs).unwrap_or(300),
                    emit_raw_events: acp_cfg.and_then(|c| c.emit_raw_events).unwrap_or(true),
                };
                (bin, config)
            };
            let info = BackendInfo {
                id: be.id().to_string(),
                capabilities: CapabilitiesView {
                    streaming: caps.streaming,
                    mcp_injection: caps.mcp_injection,
                    structured_output: caps.structured_output,
                    models: caps.models,
                },
                binary,
                config,
            };
            println!("{}", serde_json::to_string_pretty(&info).unwrap());
        }
        Err(e) => eprintln!("Error: {e}"),
    }
}

pub fn check_backend(id: Option<String>) {
    let be_id = id.unwrap_or_else(|| crate::backend::detect_backend().to_string());
    match be_id.as_str() {
        "mock" => {
            println!("\u{2713} mock backend is always available");
        }
        "codex" => {
            let cfg = crate::config::load_config();
            let binary = cfg
                .as_ref()
                .and_then(|c| c.backend.codex_acp.command.as_ref())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| {
                    if cfg!(windows) {
                        std::path::PathBuf::from("npx.cmd")
                    } else {
                        std::path::PathBuf::from("npx")
                    }
                });
            if binary.is_absolute() {
                if binary.exists() {
                    println!("\u{2713} {be_id} binary found at {}", binary.display());
                } else {
                    println!("\u{2717} {be_id} binary not found at {}", binary.display());
                }
            } else if crate::backend::which_exists(binary.to_str().unwrap_or("")) {
                println!("\u{2713} {be_id} binary found");
            } else {
                println!("\u{2717} {be_id} not found in PATH");
            }

            let handshake_timeout = cfg
                .as_ref()
                .and_then(|c| c.backend.codex_acp.connect_timeout_secs)
                .map(Duration::from_secs)
                .unwrap_or(Duration::from_secs(10));
            let path = binary.to_path_buf();
            match check_acp_handshake(&path, handshake_timeout, be_id.as_str()) {
                Ok(()) => println!("\u{2713} ACP initialize handshake succeeded"),
                Err(e) => println!("\u{2717} ACP handshake failed: {e}"),
            }
        }
        "loom-acp" | "opencode" | "claude-acp" => {
            // Check config override first, then PATH.
            let cfg = crate::config::load_config();
            let binary = cfg
                .as_ref()
                .and_then(|c| c.backend.acp.binary.as_ref())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(crate::backend::default_binary_name(&be_id))
                });
            if binary.is_absolute() {
                if binary.exists() {
                    println!("\u{2713} {be_id} binary found at {}", binary.display());
                } else {
                    println!("\u{2717} {be_id} binary not found at {}", binary.display());
                }
            } else if crate::backend::which_exists(binary.to_str().unwrap_or("")) {
                println!("\u{2713} {be_id} binary found");
            } else {
                println!("\u{2717} {be_id} not found in PATH");
            }

            // Real ACP initialize handshake to verify the binary is a working ACP agent.
            let handshake_timeout = cfg
                .as_ref()
                .and_then(|c| c.backend.acp.connect_timeout_secs)
                .map(Duration::from_secs)
                .unwrap_or(Duration::from_secs(10));
            let path = binary.to_path_buf();
            match check_acp_handshake(&path, handshake_timeout, be_id.as_str()) {
                Ok(()) => println!("\u{2713} ACP initialize handshake succeeded"),
                Err(e) => println!("\u{2717} ACP handshake failed: {e}"),
            }
        }
        other => {
            eprintln!("Unknown backend: {other}");
        }
    }
}

#[allow(dead_code)]
pub fn config_backend(key: Option<String>, value: Option<String>) {
    match (key, value) {
        (None, None) => {
            // Print current config
            let cfg = load_config().unwrap_or_default();
            println!("{}", serde_json::to_string_pretty(&cfg).unwrap());
            println!();
            println!("Config file: {}", config_path().display());
            println!("(Use `luft backend config <key> <value>` to update)");
        }
        (Some(key), Some(value)) => {
            let mut cfg = load_config().unwrap_or_default();
            if let Err(e) = apply_config_update(&mut cfg, &key, &value) {
                eprintln!("Error: {e}");
                return;
            }
            match save_config(&cfg) {
                Ok(()) => println!("\u{2713} Config saved: {key} = {value}"),
                Err(e) => eprintln!("Error: {e}"),
            }
        }
        (Some(_), None) => {
            eprintln!("Error: missing value. Usage: `luft backend config <key> <value>`");
        }
        (None, Some(_)) => {
            eprintln!("Error: missing key. Usage: `luft backend config <key> <value>`");
        }
    }
}

#[allow(dead_code)]
fn apply_config_update(cfg: &mut LuftConfig, key: &str, value: &str) -> Result<(), String> {
    match key {
        "default" => {
            cfg.backend.default = Some(value.to_string());
            Ok(())
        }
        "acp.log_level" => {
            cfg.backend.acp.log_level = Some(value.to_string());
            Ok(())
        }
        "acp.connect_timeout_secs" => {
            let n: u64 = value
                .parse()
                .map_err(|_| format!("invalid number: {value}"))?;
            cfg.backend.acp.connect_timeout_secs = Some(n);
            Ok(())
        }
        "acp.idle_timeout_secs" => {
            let n: u64 = value
                .parse()
                .map_err(|_| format!("invalid number: {value}"))?;
            cfg.backend.acp.idle_timeout_secs = Some(n);
            Ok(())
        }
        "acp.emit_raw_events" => {
            let b: bool = match value {
                "true" | "1" | "yes" => true,
                "false" | "0" | "no" => false,
                _ => return Err(format!("invalid bool: {value} (expected true/false)")),
            };
            cfg.backend.acp.emit_raw_events = Some(b);
            Ok(())
        }
        "acp.binary" => {
            cfg.backend.acp.binary = Some(value.into());
            Ok(())
        }
        "acp.args" => {
            cfg.backend.acp.args = Some(value.split(',').map(|s| s.trim().to_string()).collect());
            Ok(())
        }
        _ => Err(format!(
            "unknown config key: {key}\n  known keys: default, acp.log_level, acp.binary, \
             acp.args, acp.connect_timeout_secs, acp.idle_timeout_secs, acp.emit_raw_events"
        )),
    }
}

#[allow(dead_code)]
pub fn set_default_backend(id: String) {
    let mut cfg = load_config().unwrap_or_default();
    cfg.backend.default = Some(id.clone());
    match save_config(&cfg) {
        Ok(()) => println!("\u{2713} Config saved: default backend = \"{id}\""),
        Err(e) => eprintln!("Error: {e}"),
    }
}

// ── ACP handshake check ────────────────────────────────────────────────────

/// Spawn the binary as an ACP subprocess and perform an `initialize` handshake
/// only (no `session/new`). This verifies the binary is a real ACP agent without
/// side effects.
fn check_acp_handshake(binary: &Path, timeout: Duration, backend_id: &str) -> Result<(), String> {
    let binary = binary.to_path_buf();
    let backend_id = backend_id.to_string();
    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("runtime: {e}"))?;
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            let mut cmd = tokio::process::Command::new(&binary);
            let cfg = crate::config::load_config();
            // Backend-aware args resolution:
            //  - codex: read codex_acp.args, default to codex_default_args()
            //  - others: read acp.args, fallback to "acp" only for opencode
            let acp_args: Vec<String> = if backend_id == "codex" {
                cfg.as_ref()
                    .and_then(|c| c.backend.codex_acp.args.clone())
                    .unwrap_or_else(codex_default_args)
            } else {
                cfg.as_ref()
                    .and_then(|c| c.backend.acp.args.clone())
                    .unwrap_or_default()
            };
            if !acp_args.is_empty() {
                cmd.args(&acp_args);
            } else if backend_id == "opencode" {
                cmd.arg("acp");
            }
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut child = cmd
                .spawn()
                .map_err(|e| format!("spawn {}: {e}", binary.display()))?;
            let stdin = child.stdin.take().ok_or("no stdin")?;
            let stdout = child.stdout.take().ok_or("no stdout")?;
            let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());

            let result = tokio::time::timeout(timeout, async {
                Client
                    .builder()
                    .name("luft-check")
                    .connect_with(transport, {
                        move |conn: agent_client_protocol::ConnectionTo<
                            agent_client_protocol::Agent,
                        >| async move {
                            conn.send_request(InitializeRequest::new(ProtocolVersion::V1))
                                .block_task()
                                .await?;
                            Ok(())
                        }
                    })
                    .await
                    .map_err(|e| format!("connect: {e:?}"))
            })
            .await;

            let _ = child.start_kill().ok();
            let _ = child.wait().await;

            match result {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(format!("protocol: {e}")),
                Err(_) => Err("timed out waiting for initialize response".into()),
            }
        })
    });

    handle
        .join()
        .map_err(|_| "internal error: handshake thread panicked")?
}

fn codex_default_args() -> Vec<String> {
    vec!["-y".into(), "@agentclientprotocol/codex-acp".into()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AcpConfigOverride, BackendConfig, PlannerConfig};

    // ── apply_config_update ──────────────────────────────────────────────

    #[test]
    fn apply_config_update_default_key() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "default", "opencode").unwrap();
        assert_eq!(cfg.backend.default.as_deref(), Some("opencode"));
    }

    #[test]
    fn apply_config_update_default_key_empty_value() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "default", "").unwrap();
        assert_eq!(cfg.backend.default.as_deref(), Some(""));
    }

    #[test]
    fn apply_config_update_acp_log_level() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "acp.log_level", "debug").unwrap();
        assert_eq!(cfg.backend.acp.log_level.as_deref(), Some("debug"));
    }

    #[test]
    fn apply_config_update_acp_connect_timeout_secs_valid() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "acp.connect_timeout_secs", "30").unwrap();
        assert_eq!(cfg.backend.acp.connect_timeout_secs, Some(30));
    }

    #[test]
    fn apply_config_update_acp_connect_timeout_secs_zero() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "acp.connect_timeout_secs", "0").unwrap();
        assert_eq!(cfg.backend.acp.connect_timeout_secs, Some(0));
    }

    #[test]
    fn apply_config_update_acp_connect_timeout_secs_invalid() {
        let mut cfg = LuftConfig::default();
        let err = apply_config_update(&mut cfg, "acp.connect_timeout_secs", "abc").unwrap_err();
        assert!(err.contains("invalid number"));
        assert!(err.contains("abc"));
    }

    #[test]
    fn apply_config_update_acp_connect_timeout_secs_negative_rejected() {
        let mut cfg = LuftConfig::default();
        let err = apply_config_update(&mut cfg, "acp.connect_timeout_secs", "-1").unwrap_err();
        assert!(err.contains("invalid number"));
    }

    #[test]
    fn apply_config_update_acp_idle_timeout_secs_valid() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "acp.idle_timeout_secs", "300").unwrap();
        assert_eq!(cfg.backend.acp.idle_timeout_secs, Some(300));
    }

    #[test]
    fn apply_config_update_acp_idle_timeout_secs_invalid() {
        let mut cfg = LuftConfig::default();
        let err = apply_config_update(&mut cfg, "acp.idle_timeout_secs", "xyz").unwrap_err();
        assert!(err.contains("invalid number"));
    }

    #[test]
    fn apply_config_update_acp_emit_raw_events_true_variants() {
        for v in ["true", "1", "yes"] {
            let mut cfg = LuftConfig::default();
            apply_config_update(&mut cfg, "acp.emit_raw_events", v).unwrap();
            assert_eq!(cfg.backend.acp.emit_raw_events, Some(true), "value={v}");
        }
    }

    #[test]
    fn apply_config_update_acp_emit_raw_events_false_variants() {
        for v in ["false", "0", "no"] {
            let mut cfg = LuftConfig::default();
            apply_config_update(&mut cfg, "acp.emit_raw_events", v).unwrap();
            assert_eq!(cfg.backend.acp.emit_raw_events, Some(false), "value={v}");
        }
    }

    #[test]
    fn apply_config_update_acp_emit_raw_events_invalid() {
        let mut cfg = LuftConfig::default();
        let err = apply_config_update(&mut cfg, "acp.emit_raw_events", "maybe").unwrap_err();
        assert!(err.contains("invalid bool"));
        assert!(err.contains("maybe"));
    }

    #[test]
    fn apply_config_update_acp_binary() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "acp.binary", "/usr/local/bin/opencode").unwrap();
        assert_eq!(
            cfg.backend.acp.binary.as_deref(),
            Some(std::path::Path::new("/usr/local/bin/opencode"))
        );
    }

    #[test]
    fn apply_config_update_acp_binary_empty() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "acp.binary", "").unwrap();
        assert_eq!(
            cfg.backend.acp.binary.as_ref().map(|p| p.to_str().unwrap()),
            Some("")
        );
    }

    #[test]
    fn apply_config_update_acp_args_single() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "acp.args", "verbose").unwrap();
        assert_eq!(
            cfg.backend.acp.args.as_ref().unwrap(),
            &vec!["verbose".to_string()]
        );
    }

    #[test]
    fn apply_config_update_acp_args_multiple() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "acp.args", "verbose, no-color").unwrap();
        assert_eq!(
            cfg.backend.acp.args.as_ref().unwrap(),
            &vec!["verbose".to_string(), "no-color".to_string()]
        );
    }

    #[test]
    fn apply_config_update_acp_args_with_extra_whitespace() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "acp.args", "  a ,  b  ,c ").unwrap();
        assert_eq!(
            cfg.backend.acp.args.as_ref().unwrap(),
            &vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn apply_config_update_acp_args_empty_string_yields_one_empty_arg() {
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "acp.args", "").unwrap();
        // Empty input still produces a single empty-string element because
        // split(',') on "" yields one item.
        assert_eq!(cfg.backend.acp.args.as_ref().unwrap().len(), 1);
        assert_eq!(cfg.backend.acp.args.as_ref().unwrap()[0], "");
    }

    #[test]
    fn apply_config_update_unknown_key() {
        let mut cfg = LuftConfig::default();
        let err = apply_config_update(&mut cfg, "no.such.key", "x").unwrap_err();
        assert!(err.contains("unknown config key"));
        assert!(err.contains("no.such.key"));
        // Hint should list at least one known key.
        assert!(err.contains("default"));
    }

    #[test]
    fn apply_config_update_empty_key_is_unknown() {
        let mut cfg = LuftConfig::default();
        let err = apply_config_update(&mut cfg, "", "x").unwrap_err();
        assert!(err.contains("unknown config key"));
    }

    #[test]
    fn apply_config_update_preserves_other_fields() {
        let mut cfg = LuftConfig {
            backend: BackendConfig {
                default: Some("keep".into()),
                model: Some("keep-model".into()),
                acp: AcpConfigOverride::default(),
                codex_acp: Default::default(),
            },
            planner: PlannerConfig {
                model: Some("keep-planner".into()),
            },
        };
        apply_config_update(&mut cfg, "acp.log_level", "trace").unwrap();
        // Untouched fields remain.
        assert_eq!(cfg.backend.default.as_deref(), Some("keep"));
        assert_eq!(cfg.backend.model.as_deref(), Some("keep-model"));
        assert_eq!(cfg.planner.model.as_deref(), Some("keep-planner"));
        assert_eq!(cfg.backend.acp.log_level.as_deref(), Some("trace"));
    }

    // ── bool_mark ────────────────────────────────────────────────────────

    #[test]
    fn bool_mark_true() {
        let mark = bool_mark(true);
        assert!(!mark.is_empty());
        // Should be the check character.
        assert_ne!(mark, bool_mark(false));
    }

    #[test]
    fn bool_mark_false() {
        let mark = bool_mark(false);
        assert!(!mark.is_empty());
        assert_ne!(mark, bool_mark(true));
    }

    #[test]
    fn bool_mark_returns_static_str() {
        let a: &'static str = bool_mark(true);
        let b: &'static str = bool_mark(false);
        assert!(a.is_ascii() || !a.is_empty());
        assert!(b.is_ascii() || !b.is_empty());
    }

    // ── BackendSubcommand (clap derive compile-time + Debug) ──────────────

    #[test]
    fn backend_subcommand_list_debug() {
        let cmd = BackendSubcommand::List;
        let s = format!("{cmd:?}");
        assert!(s.contains("List"));
    }

    #[test]
    fn backend_subcommand_info_debug() {
        let cmd = BackendSubcommand::Info {
            id: Some("opencode".into()),
        };
        let s = format!("{cmd:?}");
        assert!(s.contains("Info"));
        assert!(s.contains("opencode"));
    }

    #[test]
    fn backend_subcommand_check_debug() {
        let cmd = BackendSubcommand::Check { id: None };
        let s = format!("{cmd:?}");
        assert!(s.contains("Check"));
    }

    #[test]
    fn backend_subcommand_config_debug() {
        let cmd = BackendSubcommand::Config {
            key: Some("acp.log_level".into()),
            value: Some("debug".into()),
        };
        let s = format!("{cmd:?}");
        assert!(s.contains("Config"));
        assert!(s.contains("acp.log_level"));
        assert!(s.contains("debug"));
    }

    #[test]
    fn backend_subcommand_set_debug() {
        let cmd = BackendSubcommand::Set {
            id: "opencode".into(),
        };
        let s = format!("{cmd:?}");
        assert!(s.contains("Set"));
        assert!(s.contains("opencode"));
    }

    // ── apply_config_update: combined scenarios ──────────────────────────

    #[test]
    fn apply_config_update_full_round_trip_via_save() {
        // Use a tempdir; we can't redirect config_dir(), but we can confirm
        // that all known keys are accepted and produce parseable TOML.
        let mut cfg = LuftConfig::default();
        apply_config_update(&mut cfg, "default", "opencode").unwrap();
        apply_config_update(&mut cfg, "acp.log_level", "info").unwrap();
        apply_config_update(&mut cfg, "acp.connect_timeout_secs", "5").unwrap();
        apply_config_update(&mut cfg, "acp.idle_timeout_secs", "60").unwrap();
        apply_config_update(&mut cfg, "acp.emit_raw_events", "true").unwrap();
        apply_config_update(&mut cfg, "acp.binary", "/usr/bin/opencode").unwrap();
        apply_config_update(&mut cfg, "acp.args", "x,y,z").unwrap();

        let s = toml::to_string(&cfg).unwrap();
        let parsed: LuftConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed.backend.default.as_deref(), Some("opencode"));
        assert_eq!(parsed.backend.acp.log_level.as_deref(), Some("info"));
        assert_eq!(parsed.backend.acp.connect_timeout_secs, Some(5));
        assert_eq!(parsed.backend.acp.idle_timeout_secs, Some(60));
        assert_eq!(parsed.backend.acp.emit_raw_events, Some(true));
        assert_eq!(
            parsed.backend.acp.binary.as_deref(),
            Some(std::path::Path::new("/usr/bin/opencode"))
        );
        assert_eq!(
            parsed.backend.acp.args.as_ref().unwrap(),
            &vec!["x".to_string(), "y".to_string(), "z".to_string()]
        );
    }

}
