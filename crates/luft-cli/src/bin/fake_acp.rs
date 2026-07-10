//! Minimal fake ACP agent for integration tests.
//!
//! This binary speaks enough of the Agent Client Protocol (ACP) v1 to let
//! Luft's `AcpAdapter` complete a one-shot session. It is used by tests
//! that verify `structured_output` MCP tool capture.
//!
//! Wire protocol: newline-delimited JSON-RPC 2.0.
//!
//! Supported request methods:
//! - `initialize`   -> returns protocol version v1
//! - `session/new`  -> returns session id "sess-test"
//! - `session/prompt` -> emits a `session/update` ToolCall notification with
//!   title "structured_output" and the raw input supplied via `--raw-input`,
//!   then returns a PromptResponse with stop_reason "end_turn".

use std::io::{BufRead, Write};

fn main() {
    let raw_input_arg = std::env::var("FAKE_ACP_RAW_INPUT")
        .or_else(|_| {
            std::env::args()
                .position(|a| a == "--raw-input")
                .and_then(|idx| std::env::args().nth(idx + 1))
                .ok_or(std::env::VarError::NotPresent)
        })
        .unwrap_or_else(|_| r#"{"answer":"ok"}"#.to_string());

    let raw_input: serde_json::Value = serde_json::from_str(&raw_input_arg)
        .unwrap_or_else(|_| serde_json::json!({"answer": raw_input_arg}));

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);

        match method {
            "initialize" => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "v1"
                    }
                });
                send(&mut stdout, resp);
            }
            "session/new" => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "sessionId": "sess-test"
                    }
                });
                send(&mut stdout, resp);
            }
            "session/prompt" => {
                let notif = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": "sess-test",
                        "update": {
                            "sessionUpdate": "tool_call",
                            "toolCallId": "tc-structured-output",
                            "title": "structured_output",
                            "rawInput": raw_input
                        }
                    }
                });
                send(&mut stdout, notif);

                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "sessionId": "sess-test",
                        "stopReason": "end_turn"
                    }
                });
                send(&mut stdout, resp);
            }
            "session/cancel" => {
                break;
            }
            other => {
                let _ = writeln!(stderr, "fake-acp: unknown method {other}");
            }
        }
    }
}

fn send<W: Write>(out: &mut W, msg: serde_json::Value) {
    let line = serde_json::to_string(&msg).unwrap();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    // The `main` function reads stdin/stdout/stderr and depends on env vars,
    // so it's tested as a subprocess by integration tests. Here we only
    // exercise the pure `send` helper.

    #[test]
    fn send_writes_single_line() {
        let mut buf: Vec<u8> = Vec::new();
        send(&mut buf, serde_json::json!({"jsonrpc": "2.0", "id": 1}));
        let s = String::from_utf8(buf).unwrap();
        assert!(s.ends_with('\n'), "send must terminate with newline");
        assert_eq!(s.matches('\n').count(), 1, "exactly one newline expected");
    }

    #[test]
    fn send_writes_compact_json() {
        let mut buf: Vec<u8> = Vec::new();
        send(&mut buf, serde_json::json!({"hello": "world"}));
        let s = String::from_utf8(buf).unwrap();
        let trimmed = s.trim_end_matches('\n');
        // Compact (no pretty-printed indentation).
        assert!(!trimmed.contains('\n'));
        assert!(trimmed.contains("\"hello\""));
        assert!(trimmed.contains("\"world\""));
    }

    #[test]
    fn send_writes_valid_json() {
        let mut buf: Vec<u8> = Vec::new();
        send(
            &mut buf,
            serde_json::json!({"jsonrpc": "2.0", "id": 7, "result": {"v": 1}}),
        );
        let s = String::from_utf8(buf).unwrap();
        let trimmed = s.trim_end_matches('\n');
        let parsed: serde_json::Value = serde_json::from_str(trimmed).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 7);
        assert_eq!(parsed["result"]["v"], 1);
    }

    #[test]
    fn send_flushes_writer() {
        // Cursor<Vec<u8>> wraps a Vec<u8> with a 0-length buffer; after send,
        // the inner Vec must contain the payload (i.e. flush was called).
        let mut buf = std::io::Cursor::new(Vec::<u8>::new());
        send(&mut buf, serde_json::json!({"x": 1}));
        assert!(!buf.get_ref().is_empty(), "flush should have written payload");
    }

    #[test]
    fn send_handles_null_value() {
        let mut buf: Vec<u8> = Vec::new();
        send(&mut buf, serde_json::Value::Null);
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim_end_matches('\n'), "null");
    }

    #[test]
    fn send_handles_array_value() {
        let mut buf: Vec<u8> = Vec::new();
        send(&mut buf, serde_json::json!([1, 2, 3]));
        let s = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(s.trim_end_matches('\n')).unwrap();
        assert_eq!(parsed, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn send_handles_nested_object() {
        let mut buf: Vec<u8> = Vec::new();
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "sess-test",
                "update": {
                    "sessionUpdate": "tool_call",
                    "toolCallId": "tc-1",
                    "title": "structured_output",
                    "rawInput": {"answer": "ok"}
                }
            }
        });
        send(&mut buf, payload.clone());
        let s = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(s.trim_end_matches('\n')).unwrap();
        assert_eq!(parsed, payload);
    }

    #[test]
    fn send_handles_unicode() {
        let mut buf: Vec<u8> = Vec::new();
        send(&mut buf, serde_json::json!({"msg": "héllo \u{1F600}"}));
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("héllo"));
        assert!(s.contains("\u{1F600}"));
    }

    #[test]
    fn send_never_panics_on_empty_object() {
        let mut buf: Vec<u8> = Vec::new();
        send(&mut buf, serde_json::json!({}));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim_end_matches('\n'), "{}");
    }

    #[test]
    fn send_with_multiple_calls_appends_lines() {
        let mut buf: Vec<u8> = Vec::new();
        send(&mut buf, serde_json::json!({"a": 1}));
        send(&mut buf, serde_json::json!({"b": 2}));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.matches('\n').count(), 2, "two calls -> two newlines");
        let lines: Vec<&str> = s.split('\n').filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "{\"a\":1}");
        assert_eq!(lines[1], "{\"b\":2}");
    }
}
