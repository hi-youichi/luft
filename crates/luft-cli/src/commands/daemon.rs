//! `luft daemon` subcommand — start, stop, status.

use anyhow::Result;
use clap::Subcommand;

use crate::backend;

#[derive(Debug, Subcommand)]
pub enum DaemonSubcommand {
    /// Start the daemon. Detaches to background by default.
    Start {
        /// Port to listen on.
        #[arg(long, default_value_t = luft_daemon::autostart::DEFAULT_PORT)]
        port: u16,
        /// Run in foreground (blocks terminal). Internal use.
        #[arg(long)]
        foreground: bool,
        /// Base directory for workflow run data. Defaults to `.luft/runs`.
        #[arg(long)]
        base_dir: Option<String>,
    },
    /// Stop the running daemon.
    Stop,
    /// Check if daemon is running.
    Status,
}

pub async fn run(cmd: DaemonSubcommand) -> Result<()> {
    match cmd {
        DaemonSubcommand::Start { port, foreground, base_dir } => start(port, foreground, base_dir).await,
        DaemonSubcommand::Stop => stop().await,
        DaemonSubcommand::Status => status(),
    }
}

async fn start(port: u16, foreground: bool, base_dir: Option<String>) -> Result<()> {
    if !foreground {
        return spawn_detached(port).await;
    }
    run_foreground(port, base_dir).await
}

async fn run_foreground(port: u16, base_dir: Option<String>) -> Result<()> {
    let mut reg = luft_core::scheduler::BackendRegistry::new();

    // When LUFT_MOCK_BEHAVIOR is set, force mock-only mode and skip backend
    // detection. This gives deterministic, hermetic test environments.
    let ids: Vec<&str> = if std::env::var("LUFT_MOCK_BEHAVIOR").is_ok() {
        vec!["mock"]
    } else {
        let mut ids = backend::detect_available_backends();
        if ids.is_empty() {
            ids.push("mock");
        }
        ids
    };

    for id in &ids {
        match backend::create_backend(id, false, None) {
            Ok(b) => {
                println!("  registered backend: {id}");
                reg = reg.with(b);
            }
            Err(e) => eprintln!("  failed to create backend '{id}': {e}"),
        }
    }

    println!("Daemon backends: {}", ids.join(", "));
    let mut builder = luft::Luft::builder().registry(reg);
    if let Some(dir) = &base_dir {
        builder = builder.base_dir(dir);
    }
    let luft = builder.build()?;

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    luft_daemon::serve(luft, listener).await
}

/// Resolve the daemon log directory (`~/.luft`).
fn daemon_log_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".luft")
}

/// Spawn a detached daemon process in the background.
async fn spawn_detached(port: u16) -> Result<()> {
    if let Some(pf) = luft_daemon::read_pid()? {
        if luft_daemon::is_alive(pf.pid) {
            println!("Daemon already running: PID {}, addr {}", pf.pid, pf.addr);
            return Ok(());
        }
    }

    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("daemon")
        .arg("start")
        .arg("--port")
        .arg(port.to_string())
        .arg("--foreground");

    let log_dir = daemon_log_dir();
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("daemon.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(log_file.try_clone()?))
            .stderr(std::process::Stdio::from(log_file))
            .process_group(0);
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(log_file.try_clone()?))
            .stderr(std::process::Stdio::from(log_file))
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }

    let child = cmd.spawn()?;
    drop(child);

    for _ in 0..50 {
        if let Some(pf) = luft_daemon::read_pid()? {
            if luft_daemon::is_alive(pf.pid) {
                println!("Daemon started: PID {}, addr {}", pf.pid, pf.addr);
                println!("Log: {}", log_path.display());
                return Ok(());
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!(
        "daemon did not start within 5s; check log: {}",
        log_path.display()
    )
}

async fn stop() -> Result<()> {
    let pf = luft_daemon::read_pid()?;
    match pf {
        Some(pf) => {
            let pid = pf.pid;
            println!("Stopping daemon (PID {pid})...");
            #[cfg(unix)]
            {
                unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            }
            #[cfg(windows)]
            {
                std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .status()?;
            }
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
