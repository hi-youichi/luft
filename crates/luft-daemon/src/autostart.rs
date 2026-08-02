//! Daemon auto-start: spawn a daemon child process when none is running.

use std::time::Duration;

use anyhow::{bail, Result};
use tokio_tungstenite::connect_async;
use tracing::{debug, warn};

use crate::process;

/// Default port for the daemon.
pub const DEFAULT_PORT: u16 = 7878;

/// Discover a running daemon, or auto-start one.
///
/// Returns the daemon's address (`"host:port"`) on success.
pub async fn discover_or_autostart(backend: Option<String>) -> Result<String> {
    if let Some(addr) = process::discover()? {
        if try_connect(&addr).await.is_ok() {
            debug!(%addr, "discovered running daemon");
            return Ok(addr);
        }
        warn!("stale daemon PID file, removing");
        process::remove();
    }
    autostart(backend).await
}

/// Spawn `luft daemon` as a child, then poll until it's reachable.
async fn autostart(backend: Option<String>) -> Result<String> {
    let port = std::env::var("LUFT_DAEMON_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let addr = format!("127.0.0.1:{port}");

    let exe = std::env::current_exe()?;
    debug!(exe = %exe.display(), port, "auto-starting daemon");

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("daemon")
        .arg("start")
        .arg("--port")
        .arg(port.to_string())
        .arg("--foreground");

    if let Some(ref id) = backend {
        cmd.arg("--backend").arg(id);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
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
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    cmd.spawn()?;

    // Poll with backoff: total ~10s
    let mut delay = 50u64;
    for i in 0..40 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        match try_connect(&addr).await {
            Ok(()) => {
                debug!(%addr, attempts = i, "daemon is reachable");
                return Ok(addr);
            }
            Err(_) if i < 39 => {
                delay = (delay + 50).min(500);
            }
            Err(e) => {
                warn!(error = %e, "daemon connect failed on final attempt");
            }
        }
    }
    bail!("daemon failed to start within 10s")
}

/// Perform a real WebSocket handshake against the MCP endpoint.
///
/// A raw TCP probe is not sufficient here: the daemon accepts the connection
/// as WebSocket immediately, so a TCP-only probe that closes the socket is
/// logged by the daemon as `Handshake not finished`. More importantly, TCP
/// readiness does not prove that the MCP endpoint is ready for the proxy.
async fn try_connect(addr: &str) -> Result<()> {
    let url = format!("ws://{addr}/mcp");
    let (mut ws, _response) =
        tokio::time::timeout(Duration::from_millis(750), connect_async(&url)).await??;
    // Complete the probe with a WebSocket close frame so the daemon observes
    // a normal short-lived probe rather than an unfinished handshake.
    let _ = ws.close(None).await;
    Ok(())
}
