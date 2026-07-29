//! `luft daemon` subcommand — start, stop, status.

use anyhow::Result;
use clap::Subcommand;

use crate::backend;

#[derive(Debug, Subcommand)]
pub enum DaemonSubcommand {
    /// Start the daemon (foreground, blocks until shutdown signal).
    Start {
        /// Port to listen on.
        #[arg(long, default_value_t = luft_daemon::autostart::DEFAULT_PORT)]
        port: u16,
        /// Override the default backend (agents without explicit `backend` use this).
        #[arg(long)]
        backend: Option<String>,
    },
    /// Stop the running daemon.
    Stop,
    /// Check if daemon is running.
    Status,
}

pub async fn run(cmd: DaemonSubcommand) -> Result<()> {
    match cmd {
        DaemonSubcommand::Start { port, backend } => start(port, backend).await,
        DaemonSubcommand::Stop => stop().await,
        DaemonSubcommand::Status => status(),
    }
}

async fn start(port: u16, default_backend: Option<String>) -> Result<()> {
    // Auto-detect all available backends and register them.
    let mut ids = backend::detect_available_backends();
    if ids.is_empty() {
        ids.push("mock");
    }
    let mut reg = luft_core::scheduler::BackendRegistry::new();
    for id in &ids {
        match backend::create_backend(id, false, None) {
            Ok(b) => {
                println!("  registered backend: {id}");
                reg = reg.with(b);
            }
            Err(e) => eprintln!("  failed to create backend '{id}': {e}"),
        }
    }
    // Set explicit default if requested
    if let Some(ref id) = default_backend {
        if let Ok(b) = backend::create_backend(id, false, None) {
            reg = reg.with_default(b);
        }
    }
    println!("Daemon backends: {}", ids.join(", "));
    let luft = luft::Luft::builder().registry(reg).build()?;

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    luft_daemon::serve(luft, listener).await
}

async fn stop() -> Result<()> {
    let pf = luft_daemon::read_pid()?;
    match pf {
        Some(pf) => {
            let pid = pf.pid;
            println!("Stopping daemon (PID {pid})...");
            // Send Ctrl+C on Windows, SIGTERM on Unix
            #[cfg(unix)]
            {
                unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            }
            #[cfg(windows)]
            {
                // On Windows, generate a Ctrl+C event in the daemon's process group
                std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T"])
                    .status()?;
            }
            // Wait for PID file removal
            for _ in 0..20 {
                if luft_daemon::read_pid()?.is_none() {
                    println!("Daemon stopped.");
                    return Ok(());
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            anyhow::bail!("daemon did not stop within 2s");
        }
        None => {
            println!("No daemon running.");
            Ok(())
        }
    }
}

fn status() -> Result<()> {
    match luft_daemon::read_pid()? {
        Some(pf) if luft_daemon::is_alive(pf.pid) => {
            println!("Daemon running: PID {}, addr {}", pf.pid, pf.addr);
            println!("Started: {}", pf.started_at);
            println!("Version: {}", pf.version);
        }
        Some(_) => {
            println!("Daemon not running (stale PID file found).");
        }
        None => {
            println!("Daemon not running.");
        }
    }
    Ok(())
}
