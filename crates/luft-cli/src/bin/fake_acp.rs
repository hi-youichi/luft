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
