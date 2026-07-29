//! stdio ↔ WebSocket proxy for MCP clients.
//!
//! Connects to a daemon's `/mcp` endpoint and forwards JSON-RPC
//! bidirectionally: stdin lines → WS text frames, WS text frames → stdout lines.

use std::time::Duration;

use anyhow::Result;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_tungstenite::tungstenite::Message;

/// Connect to `ws://addr/mcp` and proxy stdio ↔ WS.
///
/// Each newline-delimited line on stdin becomes one WS text frame.
/// Each WS text frame becomes one newline-delimited line on stdout.
pub async fn run_proxy(daemon_addr: &str) -> Result<()> {
    let url = format!("ws://{}/mcp", daemon_addr);
    let (ws, _response) = tokio_tungstenite::connect_async(&url).await?;
    let (mut ws_sink, mut ws_stream) = ws.split();

    let stdin_task = tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if ws_sink.send(Message::Text(trimmed.into())).await.is_err() {
                        break;
                    }
                    let _ = ws_sink.flush().await;
                }
                Err(_) => break,
            }
        }
        // Don't close ws_sink here — let stdout drain remaining responses.
    });

    let stdout_task = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(msg) = ws_stream.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let _ = stdout.write_all(text.as_bytes()).await;
                    let _ = stdout.write_all(b"\n").await;
                    let _ = stdout.flush().await;
                }
                Ok(Message::Binary(data)) => {
                    let _ = stdout.write_all(&data).await;
                    let _ = stdout.flush().await;
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    // Wait for stdin to close, then give stdout time to drain.
    stdin_task.await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), stdout_task).await;

    Ok(())
}
