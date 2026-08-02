//! `luft backend` subcommand — list, inspect, check, configure backends.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use agent_client_protocol::schema::{InitializeRequest, ProtocolVersion};
use agent_client_protocol::{ByteStreams, Client};
use clap::Subcommand;
use serde::Serialize;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};



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
    workflow_validate_schema: bool,
    session_resume: bool,
    models: Vec<String>,
}

pub fn list_backends() {
    let known_ids = &["mock", "loom-acp", "opencode", "claude-acp", "codex"];

    println!(
        "     id     \u{2502} streaming \u{2502} mcp_injection \u{2502} workflow_validate_schema \u{2502} session_resume \u{2502} models"
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
                    "  {:<9}\u{2502}       {}   \u{2502}           {}   \u{2502}                {}  \u{2502}          {}   \u{2502} {}",
                    id,
                    bool_mark(caps.streaming),
                    bool_mark(caps.mcp_injection),
                    bool_mark(caps.workflow_validate_schema),
                    bool_mark(caps.session_resume),
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
                    idle_timeout_secs: codex_cfg.and_then(|c| c.idle_timeout_secs).unwrap_or(300),
                    emit_raw_events: codex_cfg.and_then(|c| c.emit_raw_events).unwrap_or(true),
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
                    workflow_validate_schema: caps.workflow_validate_schema,
                    session_resume: caps.session_resume,
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

    // ── bool_mark ────────────────────────────────────────────────────────

    #[test]
    fn bool_mark_true() {
        let mark = bool_mark(true);
        assert!(!mark.is_empty());
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
}

