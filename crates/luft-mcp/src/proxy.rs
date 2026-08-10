//! stdio ↔ WebSocket proxy for MCP clients.
//!
//! Connects to a daemon's `/mcp` endpoint and forwards JSON-RPC
//! bidirectionally: stdin lines → WS text frames, WS text frames → stdout lines.
//!
//! Tracks in-flight JSON-RPC requests. When the daemon connection drops, sends
//! error responses for all pending requests, then auto-reconnects with
//! exponential backoff. Status messages are written to stderr.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

const MAX_RECONNECT_ATTEMPTS: u32 = 12;
const INITIAL_RECONNECT_DELAY_MS: u64 = 50;
const MAX_RECONNECT_DELAY_MS: u64 = 500;

/// Connect to `ws://addr/mcp[?backend=...]` and proxy stdio ↔ WS.
///
/// Tracks in-flight JSON-RPC requests. On daemon disconnect, sends error
/// responses for pending requests, then auto-reconnects with backoff.
pub async fn run_proxy(daemon_addr: &str, backend: Option<&str>) -> Result<()> {
    let mut reconnect_attempt = 0u32;
    let mut delay = Duration::from_millis(INITIAL_RECONNECT_DELAY_MS);

    loop {
        let url = match backend {
            Some(b) if !b.is_empty() => format!("ws://{daemon_addr}/mcp?backend={b}"),
            _ => format!("ws://{daemon_addr}/mcp"),
        };
        let ws = connect_with_retry(&url).await?;

        if reconnect_attempt > 0 {
            eprintln!("[luft] reconnected to daemon");
        }

        match run_session(ws).await {
            Ok(()) => return Ok(()),
            Err(in_flight) => {
                send_error_responses(&in_flight).await;
                reconnect_attempt += 1;
                if reconnect_attempt > MAX_RECONNECT_ATTEMPTS {
                    anyhow::bail!(
                        "daemon connection lost after {} reconnect attempts",
                        MAX_RECONNECT_ATTEMPTS
                    );
                }
                eprintln!("[luft] daemon connection lost, reconnecting...");
                tokio::time::sleep(delay).await;
                delay = (delay + Duration::from_millis(50))
                    .min(Duration::from_millis(MAX_RECONNECT_DELAY_MS));
            }
        }
    }
}

/// Send JSON-RPC error responses for in-flight requests.
async fn send_error_responses(in_flight: &[Value]) {
    if in_flight.is_empty() {
        return;
    }
    let mut stdout = tokio::io::stdout();
    for id in in_flight {
        let error = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32000,
                "message": "daemon connection lost, reconnecting..."
            }
        });
        let _ = stdout.write_all(error.to_string().as_bytes()).await;
        let _ = stdout.write_all(b"\n").await;
    }
    let _ = stdout.flush().await;
}

/// Run a single proxy session over an established WebSocket.
///
/// Returns `Ok(())` when stdin closes cleanly (client exit).
/// Returns `Err(in_flight)` when the WS connection drops unexpectedly.
async fn run_session(
    ws: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
) -> Result<(), Vec<Value>> {
    let (mut ws_sink, mut ws_stream) = ws.split();
    let in_flight: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));

    let in_flight_stdin = Arc::clone(&in_flight);
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
                    if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                        if parsed.get("method").is_some() {
                            if let Some(id) = parsed.get("id").cloned() {
                                if !id.is_null() {
                                    in_flight_stdin.lock().await.push(id);
                                }
                            }
                        }
                    }
                    if ws_sink.send(Message::Text(trimmed.into())).await.is_err() {
                        break;
                    }
                    let _ = ws_sink.flush().await;
                }
                Err(_) => break,
            }
        }
    });

    let in_flight_stdout = Arc::clone(&in_flight);
    let stdout_task = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(msg) = ws_stream.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                        if parsed.get("result").is_some() || parsed.get("error").is_some() {
                            if let Some(id) = parsed.get("id") {
                                in_flight_stdout.lock().await.retain(|v| v != id);
                            }
                        }
                    }
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

    let stdin_abort = stdin_task.abort_handle();

    match futures::future::select(stdin_task, stdout_task).await {
        futures::future::Either::Left((_, stdout_task)) => {
            let _ = tokio::time::timeout(Duration::from_secs(5), stdout_task).await;
            Ok(())
        }
        futures::future::Either::Right((_, stdin_task)) => {
            stdin_abort.abort();
            drop(stdin_task); // drop after abort
            let in_flight = in_flight.lock().await.clone();
            Err(in_flight)
        }
    }
}

/// Connect to the daemon's MCP WebSocket with a short startup retry window.
///
/// `luft mcp serve` can race daemon autostart: the PID file may already exist
/// while the listener is still completing its WebSocket accept loop. A single
/// failed handshake used to terminate the stdio MCP process, which surfaced to
/// clients only as the unhelpful `Transport closed` error.
async fn connect_with_retry(url: &str) -> Result<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>> {
    
    let mut delay = Duration::from_millis(50);
    let mut last_error = None;

    for attempt in 0..12 {
        match connect_async(url).await {
            Ok((ws, _response)) => return Ok(ws),
            Err(error) => {
                last_error = Some(error);
                if attempt == 11 {
                    break;
                }
                tokio::time::sleep(delay).await;
                delay = (delay + Duration::from_millis(50)).min(Duration::from_millis(500));
            }
        }
    }

    Err(anyhow::Error::new(
        last_error.expect("connect retry must record an error"),
    ))
    .context("failed to connect to Luft MCP daemon")
}
