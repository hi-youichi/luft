//! HTTP dashboard server — serves the web UI and REST API alongside the WS server.
//!
//! When the daemon receives a connection that is NOT a WebSocket upgrade,
//! it routes it here instead of to the MCP WS handler. The dashboard provides:
//!
//! - `GET /` — dashboard HTML page
//! - `GET /dashboard.css` — stylesheet
//! - `GET /dashboard.js` — frontend logic
//! - `GET /api/runs` — list all runs (JSON)
//! - `GET /api/runs/{run_dir}` — run status detail (JSON)
//! - `GET /api/runs/{run_dir}/events` — event log (JSON)
//! - `GET /api/runs/{run_dir}/report` — report value (JSON)

use std::path::Path;

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufStream};
use tracing::warn;

use crate::dashboard_assets;

/// Decide whether the peeked HTTP request buffer is a WebSocket upgrade.
/// Returns `true` if the `Upgrade: websocket` header is present.
pub fn is_websocket_upgrade(buf: &[u8]) -> bool {
    let text = match std::str::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => return false,
    };
    text.to_ascii_lowercase().contains("upgrade: websocket")
}

/// Handle a plain HTTP request (non-WebSocket) on a BufStream.
/// Reads the request, dispatches to the appropriate handler, and writes
/// the response back on the same stream.
pub async fn handle_http(
    stream: BufStream<tokio::net::TcpStream>,
    base_dir: &Path,
) -> Result<()> {
    let mut stream = stream;

    let mut buf = Vec::with_capacity(8192);
    loop {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 65536 {
            break;
        }
    }

    let request_text = String::from_utf8_lossy(&buf);
    let request_line = request_text.lines().next().unwrap_or("");
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let method = parts.first().copied().unwrap_or("GET");
    let path = parts.get(1).copied().unwrap_or("/");

    let (status, content_type, body) = route(method, path, base_dir).await;

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        len = body.len()
    );

    stream.write_all(response.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await?;

    Ok(())
}

async fn route(method: &str, path: &str, base_dir: &Path) -> (&'static str, &'static str, Vec<u8>) {
    if method != "GET" {
        return json_error(405, "Method not allowed");
    }

    match path {
        "/" | "" | "/index.html" => {
            let html = dashboard_assets::DASHBOARD_HTML;
            ("200 OK", "text/html; charset=utf-8", html.as_bytes().to_vec())
        }
        "/dashboard.css" => {
            let css = dashboard_assets::DASHBOARD_CSS;
            ("200 OK", "text/css; charset=utf-8", css.as_bytes().to_vec())
        }
        "/dashboard.js" => {
            let js = dashboard_assets::DASHBOARD_JS;
            ("200 OK", "application/javascript; charset=utf-8", js.as_bytes().to_vec())
        }
        "/api/runs" => match luft_core::query::list_runs(base_dir) {
            Ok(runs) => {
                let json = serde_json::to_vec(&runs).unwrap_or_else(|_| b"[]".to_vec());
                ("200 OK", "application/json", json)
            }
            Err(e) => {
                warn!(error = %e, "dashboard: list_runs failed");
                json_error(500, &format!("Internal error: {e}"))
            }
        },
        p if p.starts_with("/api/runs/") => handle_run_api(p, base_dir).await,
        "/api/health" => ("200 OK", "application/json", b"{\"status\":\"ok\"}".to_vec()),
        _ => json_error(404, "Not found"),
    }
}

async fn handle_run_api(path: &str, base_dir: &Path) -> (&'static str, &'static str, Vec<u8>) {
    let rest = &path["/api/runs/".len()..];

    let (run_dir, suffix) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx + 1..]),
        None => (rest, ""),
    };

    let run_dir = urldecode(run_dir);

    if run_dir.is_empty() {
        return json_error(400, "Missing run directory");
    }

    match suffix {
        "" => match luft_core::query::get_status(&run_dir, base_dir) {
            Ok(Some(status)) => {
                let json = serde_json::to_vec(&status).unwrap_or_else(|_| b"{}".to_vec());
                ("200 OK", "application/json", json)
            }
            Ok(None) => json_error(404, "Run not found"),
            Err(e) => {
                warn!(error = %e, run = %run_dir, "dashboard: get_status failed");
                json_error(500, &format!("Internal error: {e}"))
            }
        },
        "events" => match luft_core::query::get_events(&run_dir, base_dir) {
            Ok(events) => {
                let json = serde_json::to_vec(&events).unwrap_or_else(|_| b"[]".to_vec());
                ("200 OK", "application/json", json)
            }
            Err(e) => {
                warn!(error = %e, run = %run_dir, "dashboard: get_events failed");
                json_error(500, &format!("Internal error: {e}"))
            }
        },
        "report" => match luft_core::query::get_report(&run_dir, base_dir) {
            Ok(luft_core::query::ReportStatus::Found(value)) => {
                let body = serde_json::json!({"found": true, "value": value});
                let json = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
                ("200 OK", "application/json", json)
            }
            Ok(luft_core::query::ReportStatus::RunFinished) => (
                "200 OK",
                "application/json",
                serde_json::json!({"found": false, "run_finished": true}).to_string().into_bytes(),
            ),
            Ok(luft_core::query::ReportStatus::NotFound) => (
                "200 OK",
                "application/json",
                serde_json::json!({"found": false, "run_finished": false}).to_string().into_bytes(),
            ),
            Err(e) => {
                warn!(error = %e, run = %run_dir, "dashboard: get_report failed");
                json_error(500, &format!("Internal error: {e}"))
            }
        },
        _ => json_error(404, "Unknown endpoint"),
    }
}

fn json_error(code: u16, msg: &str) -> (&'static str, &'static str, Vec<u8>) {
    let status = match code {
        400 => "400 Bad Request",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        500 => "500 Internal Server Error",
        _ => "500 Internal Server Error",
    };
    let body = serde_json::json!({"error": msg}).to_string().into_bytes();
    (status, "application/json", body)
}

fn urldecode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_to_u8(bytes[i + 1]);
            let lo = hex_to_u8(bytes[i + 2]);
            if let (Some(h), Some(l)) = (hi, lo) {
                result.push((h << 4 | l) as char);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            result.push(' ');
        } else {
            result.push(bytes[i] as char);
        }
        i += 1;
    }
    result
}

fn hex_to_u8(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urldecode_basic() {
        assert_eq!(urldecode("hello"), "hello");
        assert_eq!(urldecode("hello%20world"), "hello world");
        assert_eq!(urldecode("a+b"), "a b");
    }

    #[test]
    fn ws_upgrade_detection() {
        assert!(is_websocket_upgrade(
            b"GET /mcp HTTP/1.1\r\nUpgrade: websocket\r\n\r\n"
        ));
        assert!(!is_websocket_upgrade(
            b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"
        ));
        assert!(!is_websocket_upgrade(b"garbage"));
    }

    #[test]
    fn hex_conversion() {
        assert_eq!(hex_to_u8(b'0'), Some(0));
        assert_eq!(hex_to_u8(b'F'), Some(15));
        assert_eq!(hex_to_u8(b'g'), None);
    }
}
