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
        /// Backend id (auto-detected if omitted).
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

async fn start(port: u16, backend: Option<String>) -> Result<()> {
    let backend_id = match &backend {
        Some(b) => b.as_str(),
        None => backend::detect_backend(),
    };
    let backend = backend::create_backend(backend_id, false, None)?;
    let luft = luft::Luft::builder().backend_arc(backend).build()?;

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
