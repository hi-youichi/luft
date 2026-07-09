//! `maestro backend` subcommand — list, inspect, check, configure backends.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use agent_client_protocol::schema::{InitializeRequest, ProtocolVersion};
use agent_client_protocol::{ByteStreams, Client};
use clap::Subcommand;
use serde::Serialize;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::config::{config_path, load_config, save_config, MaestroConfig};

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
    let known_ids = &["mock", "loom-acp", "opencode"];

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
                    if *id == "opencode" || *id == "loom-acp" {
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
            let binary = match be_id.as_str() {
                "mock" => "(built-in)".into(),
                _ => cfg
                    .as_ref()
                    .and_then(|c| c.backend.acp.binary.as_ref())
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| be_id.clone()),
            };
            let acp_cfg = cfg.as_ref().map(|c| &c.backend.acp);
            let info = BackendInfo {
                id: be.id().to_string(),
                capabilities: CapabilitiesView {
                    streaming: caps.streaming,
                    mcp_injection: caps.mcp_injection,
                    structured_output: caps.structured_output,
                    models: caps.models,
                },
                binary,
                config: ConfigView {
                    args: acp_cfg.and_then(|c| c.args.clone()).unwrap_or_default(),
                    log_level: acp_cfg.and_then(|c| c.log_level.clone()),
                    connect_timeout_secs: acp_cfg
                        .and_then(|c| c.connect_timeout_secs)
                        .unwrap_or(10),
                    idle_timeout_secs: acp_cfg.and_then(|c| c.idle_timeout_secs).unwrap_or(300),
                    emit_raw_events: acp_cfg.and_then(|c| c.emit_raw_events).unwrap_or(true),
                },
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
        "loom-acp" | "opencode" => {
            // Check config override first, then PATH.
            let cfg = crate::config::load_config();
            let binary = cfg
                .as_ref()
                .and_then(|c| c.backend.acp.binary.as_ref())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from(be_id.as_str()));
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
            println!("(Use `maestro backend config <key> <value>` to update)");
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
            eprintln!("Error: missing value. Usage: `maestro backend config <key> <value>`");
        }
        (None, Some(_)) => {
            eprintln!("Error: missing key. Usage: `maestro backend config <key> <value>`");
        }
    }
}

#[allow(dead_code)]
fn apply_config_update(cfg: &mut MaestroConfig, key: &str, value: &str) -> Result<(), String> {
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
            let acp_args = cfg
                .as_ref()
                .and_then(|c| c.backend.acp.args.clone())
                .unwrap_or_default();
            if !acp_args.is_empty() {
                cmd.args(&acp_args);
            } else if backend_id != "loom-acp" {
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
                    .name("maestro-check")
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
