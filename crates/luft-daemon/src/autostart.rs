//! Daemon auto-start: spawn a detached daemon child process when none is running.

use std::time::Duration;

use anyhow::{bail, Result};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;

use crate::process;

/// Default port for the daemon.
pub const DEFAULT_PORT: u16 = 7878;

/// Discover a running daemon, or auto-start one.
///
/// Returns the daemon's address (`"host:port"`) on success.
pub async fn discover_or_autostart() -> Result<String> {
    if let Some(addr) = process::discover()? {
        if try_connect(&addr).await.is_ok() {
            return Ok(addr);
        }
        process::remove();
    }
    autostart().await
}

/// Spawn `luft daemon` as a detached child, then poll until it's reachable.
async fn autostart() -> Result<String> {
    let port = std::env::var("LUFT_DAEMON_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let addr = format!("127.0.0.1:{port}");

    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("daemon").arg("--port").arg(port.to_string());

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
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    cmd.spawn()?;

    // Poll WS connect with backoff
    let mut delay = 10u64;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        if try_connect(&addr).await.is_ok() {
            return Ok(addr);
        }
        delay = (delay * 2).min(500);
    }
    bail!("daemon failed to start within 5s")
}

/// Quick WS liveness check: connect, send a ping, close.
async fn try_connect(addr: &str) -> Result<()> {
    let stream = TcpStream::connect(addr).await?;
    let ws = tokio_tungstenite::client_async(
        format!(
            "GET / HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            generate_key()
        ),
        stream,
    )
    .await?
    .0;
    drop(ws);
    Ok(())
}
