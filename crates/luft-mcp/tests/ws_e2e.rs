//! WebSocket-level E2E: full MCP protocol (initialize → tools/list → tools/call)
//! against a real `LuftMcpServer` over WebSocket via `ws_transport::serve_ws`.
//!
//! Each test spins up a TCP listener on a random port, accepts WS connections,
//! and serves the MCP server. The test acts as a WS client performing the
//! complete MCP handshake and exercising tools end-to-end.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use luft_core::{MockBackend, MockBehavior, TokenUsage};
use luft_mcp::{ws_transport, LuftMcpServer};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Message};

type ClientWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

// ── Test fixtures ────────────────────────────────────────────────────

fn make_server(behaviors: Vec<MockBehavior>) -> LuftMcpServer {
    let backend = MockBackend::new("mock", behaviors);
    let luft = luft::Luft::builder()
        .backend(backend)
        .base_dir(tempfile::TempDir::new().unwrap().keep())
        .build()
        .unwrap();
    LuftMcpServer::new(luft)
}

fn ok_behaviors(n: usize) -> Vec<MockBehavior> {
    std::iter::repeat_n(
        MockBehavior::Success {
            output: json!({"result": "ok"}),
            tokens: TokenUsage {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
            },
            delay: Duration::from_millis(1),
        },
        n,
    )
    .collect()
}

/// Start a WS server on a random port. Returns the WS URL.
///
/// The server accepts multiple connections; each gets a fresh clone of the
/// `LuftMcpServer` with no per-connection backend override.
async fn spawn_ws_server(server: LuftMcpServer) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = Arc::new(server);

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let server = Arc::clone(&server);
            tokio::spawn(async move {
                let ws = accept_async(stream).await.unwrap();
                let s = server.with_fresh_client_name_and_backend(None);
                let _ = ws_transport::serve_ws(s, ws).await;
            });
        }
    });

    format!("ws://{addr}/mcp")
}

// ── MCP client helpers ───────────────────────────────────────────────

/// Perform the MCP initialize handshake. Returns the server's info.
async fn mcp_initialize(ws: &mut ClientWs) -> Value {
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "e2e-test", "version": "1.0.0"}
        }
    });
    ws.send(Message::Text(init_req.to_string().into()))
        .await
        .unwrap();

    let resp = recv_json(ws).await;
    assert!(
        resp.get("result").is_some(),
        "initialize failed: {resp}"
    );

    // Send initialized notification — no response expected
    let notif = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    ws.send(Message::Text(notif.to_string().into()))
        .await
        .unwrap();

    resp["result"]["serverInfo"].clone()
}

async fn send_json(ws: &mut ClientWs, msg: Value) {
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .unwrap();
}

async fn recv_json(ws: &mut ClientWs) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(15), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    return v;
                }
            }
            Ok(Some(Ok(Message::Binary(b)))) => {
                if let Ok(v) = serde_json::from_slice::<Value>(&b) {
                    return v;
                }
            }
            Ok(Some(Ok(_))) => continue,
            other => panic!("WS stream ended unexpectedly: {other:?}"),
        }
    }
}

/// Call a tool and return the raw `CallToolResult`.
async fn call_tool(ws: &mut ClientWs, id: i64, name: &str, args: Value) -> Value {
    send_json(
        ws,
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
               "params": {"name": name, "arguments": args}}),
    )
    .await;
    let resp = recv_json(ws).await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("tool '{name}' returned error: {resp}"));
    assert!(
        !result.get("isError").and_then(|e| e.as_bool()).unwrap_or(false),
        "tool '{name}' returned isError=true: {result}"
    );
    result.clone()
}

/// Extract text from `CallToolResult.content[0].text`.
fn tool_text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Parse the tool result text as JSON.
fn tool_json(result: &Value) -> Value {
    let text = tool_text(result);
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("failed to parse tool result as JSON: {text}: {e}"))
}

/// Connect + initialize in one step.
async fn connect_and_init(url: &str) -> ClientWs {
    let (mut ws, _) = connect_async(url).await.unwrap();
    let info = mcp_initialize(&mut ws).await;
    assert_eq!(info["name"], "luft");
    ws
}

/// Poll `workflow_status` until terminal. Returns the final status JSON.
async fn poll_until_done(ws: &mut ClientWs, run_id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let r = call_tool(ws, 99, "workflow_status", json!({"run_id": run_id})).await;
            let s = tool_json(&r);
            match s["status"].as_str() {
                Some("running") => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                _ => return s,
            }
        }
    })
    .await
    .expect("workflow should reach terminal state within 15s")
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ws_mcp_initialize_handshake() {
    let url = spawn_ws_server(make_server(ok_behaviors(1))).await;

    // First connection
    let (mut ws, _) = connect_async(&url).await.unwrap();
    let info = mcp_initialize(&mut ws).await;
    assert_eq!(info["name"], "luft");
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    ws.close(None).await.ok();

    // Second connection on same server (multiple clients)
    let (mut ws2, _) = connect_async(&url).await.unwrap();
    let info2 = mcp_initialize(&mut ws2).await;
    assert_eq!(info2["name"], "luft");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ws_mcp_tools_list() {
    let url = spawn_ws_server(make_server(ok_behaviors(1))).await;
    let mut ws = connect_and_init(&url).await;

    send_json(
        &mut ws,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    )
    .await;
    let resp = recv_json(&mut ws).await;

    let tools: Vec<String> = resp["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();

    for expected in [
        "workflow_execute",
        "workflow_status",
        "workflow_events",
        "workflow_cancel",
        "workflow_list_files",
        "workflow_list_runs",
        "workflow_validate_schema",
    ] {
        assert!(
            tools.contains(&expected.to_string()),
            "missing tool '{expected}', got: {tools:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ws_mcp_workflow_list_files() {
    let url = spawn_ws_server(make_server(ok_behaviors(1))).await;
    let mut ws = connect_and_init(&url).await;

    let result = call_tool(&mut ws, 2, "workflow_list_files", json!({})).await;
    let files = tool_json(&result);
    assert!(files.is_array(), "expected array, got: {files}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_mcp_execute_and_status() {
    let url = spawn_ws_server(make_server(ok_behaviors(1))).await;
    let mut ws = connect_and_init(&url).await;

    let script = r#"
        meta = { reasoning = "e2e ws test", phases = { { label = "work" } } }
        function main()
            phase("work")
            local r = agent({ prompt = "do work", model = "mock" })
            report({ ok = r.ok })
        end
    "#;

    let result = call_tool(
        &mut ws,
        2,
        "workflow_execute",
        json!({"script": script, "concurrency": 1, "backend": "mock"}),
    )
    .await;
    let exec = tool_json(&result);
    let run_id = exec["run_id"].as_str().unwrap().to_string();
    assert_eq!(exec["status"], "running");

    let status = poll_until_done(&mut ws, &run_id).await;
    assert_eq!(status["status"], "completed");
    assert!(status["completed_agents"].as_u64().unwrap_or(0) > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_mcp_execute_and_events() {
    let url = spawn_ws_server(make_server(ok_behaviors(1))).await;
    let mut ws = connect_and_init(&url).await;

    let script = r#"
        meta = { reasoning = "events e2e", phases = { { label = "work" } } }
        function main()
            phase("work")
            local r = agent({ prompt = "work", model = "mock" })
            report({ ok = r.ok })
        end
    "#;

    let result = call_tool(
        &mut ws,
        2,
        "workflow_execute",
        json!({"script": script, "concurrency": 1, "backend": "mock"}),
    )
    .await;
    let run_id = tool_json(&result)["run_id"].as_str().unwrap().to_string();

    poll_until_done(&mut ws, &run_id).await;

    let result = call_tool(
        &mut ws,
        3,
        "workflow_events",
        json!({"run_id": &run_id}),
    )
    .await;
    let events = tool_json(&result);
    assert!(events["total_matching"].as_u64().unwrap_or(0) > 0);
    assert!(!events["events"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_mcp_execute_and_list_runs() {
    let url = spawn_ws_server(make_server(ok_behaviors(1))).await;
    let mut ws = connect_and_init(&url).await;

    let script = "meta = { reasoning = \"list runs\", phases = {} }\nfunction main()\n  report({ done = true })\nend";

    let result = call_tool(
        &mut ws,
        2,
        "workflow_execute",
        json!({"script": script, "concurrency": 1, "backend": "mock"}),
    )
    .await;
    let run_id = tool_json(&result)["run_id"].as_str().unwrap().to_string();

    // poll_until_done returns the status JSON which contains the UUID run_id
    let status = poll_until_done(&mut ws, &run_id).await;
    let uuid = status["run_id"].as_str().unwrap().to_string();

    let result = call_tool(&mut ws, 3, "workflow_list_runs", json!({})).await;
    let list = tool_json(&result);
    assert!(list["count"].as_u64().unwrap_or(0) > 0);
    assert!(
        list["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["run_id"] == uuid),
        "run {} not in list_runs: {}",
        uuid,
        list["runs"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_mcp_execute_cancel() {
    let url = spawn_ws_server(make_server(vec![MockBehavior::Hang])).await;
    let mut ws = connect_and_init(&url).await;

    let script = r#"
        meta = { reasoning = "cancel e2e", phases = { { label = "work" } } }
        function main()
            phase("work", 1)
            local r = agent({ prompt = "hang", model = "mock" })
            report(r)
        end
    "#;

    let result = call_tool(
        &mut ws,
        2,
        "workflow_execute",
        json!({"script": script, "concurrency": 1, "backend": "mock"}),
    )
    .await;
    let run_id = tool_json(&result)["run_id"].as_str().unwrap().to_string();

    // Wait for the run to be observable
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let r = call_tool(&mut ws, 3, "workflow_status", json!({"run_id": &run_id})).await;
            let s = tool_json(&r);
            if s["status"] == "running" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("run should become observable");

    let result = call_tool(
        &mut ws,
        4,
        "workflow_cancel",
        json!({"run_id": &run_id}),
    )
    .await;
    let cancel = tool_json(&result);
    assert_eq!(cancel["result"], "cancelling");

    // Verify run reaches cancelled state
    let status = poll_until_done(&mut ws, &run_id).await;
    assert_eq!(status["status"], "cancelled");
}
