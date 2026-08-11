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

    // Cache the MCP initialize handshake so we can replay it after a daemon
    // reconnection. Without this, the daemon's rmcp server waits forever for
    // `initialize` on the new WS connection while the MCP client (which already
    // completed the handshake) keeps sending tool calls — deadlocking the proxy.
    let cached_init_request: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let cached_init_notification: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // Single BufReader<Stdin> persisted across reconnections.  Creating a new
    // one per session (and aborting the old stdin task) corrupts stdin state on
    // Windows because the OS-level pipe handle is shared.
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);

    tracing::info!(daemon_addr = %daemon_addr, backend = ?backend, "MCP proxy starting");

    loop {
        let url = match backend {
            Some(b) if !b.is_empty() => format!("ws://{daemon_addr}/mcp?backend={b}"),
            _ => format!("ws://{daemon_addr}/mcp"),
        };
        let ws = connect_with_retry(&url).await?;
        tracing::debug!(url = %url, "connected to daemon");

        if reconnect_attempt > 0 {
            tracing::info!(attempt = reconnect_attempt, "reconnected to daemon");
        }

        match run_session(
            ws,
            &mut reader,
            &cached_init_request,
            &cached_init_notification,
        )
        .await
        {
            Ok(()) => {
                tracing::info!("proxy session ended cleanly");
                return Ok(());
            }
            Err(in_flight) => {
                send_error_responses(&in_flight).await;
                reconnect_attempt += 1;
                if reconnect_attempt > MAX_RECONNECT_ATTEMPTS {
                    tracing::error!(attempts = reconnect_attempt, "daemon connection lost, giving up");
                    anyhow::bail!(
                        "daemon connection lost after {} reconnect attempts",
                        MAX_RECONNECT_ATTEMPTS
                    );
                }
                tracing::warn!(
                    attempt = reconnect_attempt,
                    in_flight = in_flight.len(),
                    "daemon connection lost, reconnecting"
                );
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
///
/// Uses `tokio::select!` instead of spawning child tasks. This avoids the
/// Windows-specific issue where aborting a task that's reading from stdin
/// leaves the stdin pipe handle in an unusable state.
async fn run_session(
    ws: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    reader: &mut BufReader<tokio::io::Stdin>,
    cached_init_request: &Arc<Mutex<Option<String>>>,
    cached_init_notification: &Arc<Mutex<Option<String>>>,
) -> Result<(), Vec<Value>> {
    let (mut ws_sink, mut ws_stream) = ws.split();
    let mut in_flight: Vec<Value> = Vec::new();

    // ── Replay cached MCP handshake on reconnection ───────────────────
    //
    // After a daemon reconnect, rmcp expects `initialize` as the very first
    // message on the new WS connection. The MCP client won't re-send it
    // (the handshake already succeeded from its perspective), so the proxy
    // must replay the cached `initialize` + `notifications/initialized`
    // before resuming the normal relay.
    {
        let init_req = cached_init_request.lock().await.clone();
        if let Some(req_json) = &init_req {
            tracing::info!("replaying cached initialize request after reconnection");
            if ws_sink
                .send(Message::Text(req_json.clone().into()))
                .await
                .is_err()
            {
                tracing::warn!("failed to replay initialize request");
                return Err(in_flight);
            }
            let _ = ws_sink.flush().await;

            // Swallow the initialize response from the daemon. We match on
            // the request id to avoid swallowing unrelated messages.
            let init_id = serde_json::from_str::<Value>(req_json)
                .ok()
                .and_then(|v| v.get("id").cloned());

            let mut got_response = false;
            while let Some(msg) = ws_stream.next().await {
                match msg {
                    Ok(Message::Text(t)) => {
                        if let Ok(parsed) = serde_json::from_str::<Value>(&t) {
                            if init_id
                                .as_ref()
                                .is_some_and(|id| parsed.get("id") == Some(id))
                            {
                                got_response = true;
                                break;
                            }
                        }
                    }
                    Ok(Message::Binary(b)) => {
                        if let Ok(parsed) = serde_json::from_slice::<Value>(&b) {
                            if init_id
                                .as_ref()
                                .is_some_and(|id| parsed.get("id") == Some(id))
                            {
                                got_response = true;
                                break;
                            }
                        }
                    }
                    _ => break,
                }
            }

            if !got_response {
                tracing::warn!("daemon closed during initialize replay");
                return Err(in_flight);
            }

            // Replay the initialized notification
            let init_notif = cached_init_notification.lock().await.clone();
            if let Some(notif_json) = &init_notif {
                let _ = ws_sink
                    .send(Message::Text(notif_json.clone().into()))
                    .await;
                let _ = ws_sink.flush().await;
            }
            tracing::info!("MCP handshake replay complete");
        }
    }

    // ── Main relay loop: stdin ↔ WS via select! ───────────────────────
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        tokio::select! {
            // stdin → WS
            result = reader.read_line(&mut line) => {
                match result {
                    Ok(0) => {
                        tracing::debug!("stdin EOF, draining stdout");
                        // Drain any remaining WS messages for up to 5s
                        let drain = async {
                            while let Some(msg) = ws_stream.next().await {
                                if let Ok(Message::Text(text)) = msg {
                                    let _ = stdout.write_all(text.as_bytes()).await;
                                    let _ = stdout.write_all(b"\n").await;
                                    let _ = stdout.flush().await;
                                } else {
                                    break;
                                }
                            }
                        };
                        let _ = tokio::time::timeout(Duration::from_secs(5), drain).await;
                        tracing::debug!("proxy session ended (stdin EOF)");
                        return Ok(());
                    }
                    Ok(_) => {
                        let trimmed = line.trim_end();
                        if !trimmed.is_empty() {
                            if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                                // Cache MCP handshake messages for reconnection replay
                                if let Some(method) =
                                    parsed.get("method").and_then(|m| m.as_str())
                                {
                                    match method {
                                        "initialize" => {
                                            *cached_init_request.lock().await =
                                                Some(trimmed.to_string());
                                        }
                                        "notifications/initialized" => {
                                            *cached_init_notification.lock().await =
                                                Some(trimmed.to_string());
                                        }
                                        _ => {}
                                    }
                                }
                                // Track in-flight JSON-RPC requests
                                if parsed.get("method").is_some() {
                                    if let Some(id) = parsed.get("id").cloned() {
                                        if !id.is_null() {
                                            in_flight.push(id);
                                        }
                                    }
                                }
                            }
                            if ws_sink
                                .send(Message::Text(trimmed.into()))
                                .await
                                .is_err()
                            {
                                tracing::warn!("ws send failed during relay");
                                return Err(in_flight);
                            }
                            let _ = ws_sink.flush().await;
                        }
                        line.clear();
                    }
                    Err(_) => {
                        tracing::debug!("stdin read error, ending session");
                        return Ok(());
                    }
                }
            }
            // WS → stdout
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                            if parsed.get("result").is_some()
                                || parsed.get("error").is_some()
                            {
                                if let Some(id) = parsed.get("id") {
                                    in_flight.retain(|v| v != id);
                                }
                            }
                        }
                        let _ = stdout.write_all(text.as_bytes()).await;
                        let _ = stdout.write_all(b"\n").await;
                        let _ = stdout.flush().await;
                    }
                    Some(Ok(Message::Binary(data))) => {
                        let _ = stdout.write_all(&data).await;
                        let _ = stdout.flush().await;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => {
                        tracing::warn!("daemon WebSocket closed unexpectedly");
                        return Err(in_flight);
                    }
                }
            }
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
                tracing::debug!(attempt = attempt + 1, error = %error, "daemon connect attempt failed");
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
