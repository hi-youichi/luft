//! WebSocket server: TCP accept loop, WS upgrade, route `/mcp` and `/run`.

use std::sync::Arc;

use anyhow::Result;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::{info, warn};

use luft::Luft;
use luft_mcp::LuftMcpServer;

use crate::process;

/// Start the daemon server. Blocks until shutdown signal.
pub async fn serve(luft: Luft, listener: TcpListener) -> Result<()> {
    let addr = listener.local_addr()?;
    let pid = std::process::id();
    process::write(pid, &addr.to_string())?;
    info!(%addr, pid, "daemon started");

    // LuftMcpServer takes ownership of Luft but is Clone (stores it in Arc internally).
    let mcp_server = Arc::new(LuftMcpServer::new(luft));

    let mut shutdown_rx = setup_shutdown_handler();

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown_rx => {
                info!("shutdown signal received");
                break;
            }
            accept = listener.accept() => {
                let (stream, peer) = match accept {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "accept failed");
                        continue;
                    }
                };
                let server = mcp_server.with_fresh_client_name();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, peer, server).await {
                        warn!(%peer, error = %e, "connection ended with error");
                    }
                });
            }
        }
    }

    process::remove();
    info!("daemon stopped");
    Ok(())
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    mcp_server: LuftMcpServer,
) -> Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;

    // All connections route to MCP for now.
    // TODO: use accept_hdr to extract the request URI and route /mcp vs /run.
    info!(%peer, "connection routed to /mcp");
    luft_mcp::ws_transport::serve_ws(mcp_server, ws_stream).await?;
    Ok(())
}

fn setup_shutdown_handler() -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate()).unwrap();
            let mut int = signal(SignalKind::interrupt()).unwrap();
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await.ok();
        }
        let _ = tx.send(());
    });
    rx
}
