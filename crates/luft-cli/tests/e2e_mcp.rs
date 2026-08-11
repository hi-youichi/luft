//! E2E tests for `luft mcp serve` (stdio MCP proxy).
//!
//! Only spawns `luft mcp serve`; the daemon is autostarted by the proxy's
//! `discover_or_autostart()` logic. Each test isolates via:
//! - `LUFT_HOME=<tempdir>` — PID file isolation
//! - `current_dir=<tempdir>` — run data isolation (daemon defaults to `.luft/runs`)

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};
use serial_test::serial;

const BINARY: &str = env!("CARGO_BIN_EXE_luft");
const TEST_PORT: u16 = 18799;

// ── Test fixture ─────────────────────────────────────────────────────

struct McpFixture {
    proxy: Child,
    rx: std::sync::mpsc::Receiver<String>,
    home_dir: tempfile::TempDir,
}

impl McpFixture {
    async fn spawn(mock_behavior: &str) -> Self {
        // Ensure no stale daemon from a previous test is on the port.
        kill_port(TEST_PORT);

        let home_dir = tempfile::TempDir::new().unwrap();
        let work_dir = tempfile::TempDir::new().unwrap();
        let port_str = TEST_PORT.to_string();

        let common_env: Vec<(&str, &str)> = vec![
            ("LUFT_HOME", home_dir.path().to_str().unwrap()),
            ("LUFT_MOCK_BEHAVIOR", mock_behavior),
            ("LUFT_DAEMON_PORT", &port_str),
        ];

        // Only spawn the proxy — it autostarts the daemon via discover_or_autostart().
        // CWD is set to work_dir so the daemon's `.luft/runs` is isolated.
        let mut proxy = Command::new(BINARY)
            .args(["mcp", "serve", "--backend", "mock"])
            .current_dir(work_dir.path())
            .env_clear()
            .envs(env_keep())
            .envs(common_env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn luft mcp serve");

        let stdout = proxy.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => { if tx.send(line.clone()).is_err() { break; } }
                }
            }
        });

        // Keep work_dir alive for the test duration
        std::mem::forget(work_dir);

        Self { proxy, rx, home_dir }
    }

    /// Read the daemon PID from the PID file.
    fn daemon_pid(&self) -> Option<u32> {
        let pid_file = self.home_dir.path().join("daemon.pid");
        let data = std::fs::read_to_string(&pid_file).ok()?;
        serde_json::from_str::<Value>(&data)
            .ok()?
            .get("pid")?
            .as_u64()
            .map(|p| p as u32)
    }

    /// Kill the daemon by PID (for reconnect tests).
    fn kill_daemon(&self) {
        if let Some(pid) = self.daemon_pid() {
            #[cfg(windows)]
            let _ = Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).output();
            #[cfg(not(windows))]
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
        }
    }

    fn shutdown(&mut self) {
        self.kill_daemon();
        let _ = self.proxy.kill();
        let _ = self.proxy.wait();
        // Make sure port is free for the next test
        kill_port(TEST_PORT);
    }
}

/// Kill any process listening on the given port.
#[cfg(windows)]
fn kill_port(port: u16) {
    let _ = Command::new("cmd").args(["/C", &format!("for /f \"tokens=5\" %a in ('netstat -ano ^| findstr :{port} ^| findstr LISTENING') do taskkill /F /PID %a")]).output();
}

#[cfg(not(windows))]
fn kill_port(port: u16) {
    let _ = Command::new("sh").args(["-c", &format!("lsof -ti:{port} | xargs kill -9 2>/dev/null")]).output();
}

impl Drop for McpFixture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn env_keep() -> Vec<(&'static str, String)> {
    let mut v = vec![];
    if let Ok(p) = std::env::var("PATH") { v.push(("PATH", p)); }
    if let Ok(h) = std::env::var("HOME") { v.push(("HOME", h)); }
    if let Ok(h) = std::env::var("USERPROFILE") { v.push(("USERPROFILE", h)); }
    #[cfg(windows)]
    {
        for key in ["SYSTEMROOT", "TEMP", "TMP", "APPDATA", "LOCALAPPDATA"] {
            if let Ok(val) = std::env::var(key) { v.push((key, val)); }
        }
    }
    v
}

// ── JSON-RPC client helpers ──────────────────────────────────────────

fn send_rpc(proxy: &mut Child, msg: &Value) {
    let line = format!("{}\n", msg);
    let stdin = proxy.stdin.as_mut().expect("proxy stdin");
    stdin.write_all(line.as_bytes()).expect("write stdin");
    stdin.flush().expect("flush stdin");
}

fn recv_rpc(rx: &std::sync::mpsc::Receiver<String>, timeout_secs: u64) -> Value {
    let line = rx.recv_timeout(Duration::from_secs(timeout_secs))
        .unwrap_or_else(|_| panic!("timeout ({timeout_secs}s)"));
    let trimmed = line.trim();
    if trimmed.is_empty() { panic!("proxy stdout closed"); }
    serde_json::from_str(trimmed)
        .unwrap_or_else(|e| panic!("bad JSON-RPC: {trimmed}: {e}"))
}

fn rpc(proxy: &mut Child, rx: &std::sync::mpsc::Receiver<String>, msg: Value) -> Value {
    send_rpc(proxy, &msg);
    recv_rpc(rx, 30)
}

fn mcp_init(proxy: &mut Child, rx: &std::sync::mpsc::Receiver<String>) -> Value {
    let resp = rpc(proxy, rx, json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "e2e-test", "version": "1.0.0"}
        }
    }));
    assert!(resp.get("result").is_some(), "initialize failed: {resp}");
    send_rpc(proxy, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    resp["result"]["serverInfo"].clone()
}

fn call_tool(proxy: &mut Child, rx: &std::sync::mpsc::Receiver<String>, id: i64, name: &str, args: Value) -> Value {
    let resp = rpc(proxy, rx, json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {"name": name, "arguments": args}
    }));
    let result = resp.get("result")
        .unwrap_or_else(|| panic!("tool '{name}' error: {resp}"));
    assert!(!result.get("isError").and_then(|e| e.as_bool()).unwrap_or(false),
        "tool '{name}' isError: {result}");
    result.clone()
}

fn tool_json(result: &Value) -> Value {
    let text = result["content"][0]["text"].as_str().unwrap_or("");
    serde_json::from_str(text).unwrap_or_else(|e| panic!("bad tool JSON: {text}: {e}"))
}

fn poll_until_done(proxy: &mut Child, rx: &std::sync::mpsc::Receiver<String>, run_id: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if tokio::time::Instant::now() >= deadline {
            panic!("workflow {run_id} timeout 30s");
        }
        let r = call_tool(proxy, rx, 99, "workflow_status", json!({"run_id": run_id}));
        let s = tool_json(&r);
        if s["status"].as_str() != Some("running") { return s; }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn e2e_initialize_and_tools_list() {
    let mut fx = McpFixture::spawn("success").await;
    let info = mcp_init(&mut fx.proxy, &fx.rx);
    assert_eq!(info["name"], "luft");

    let resp = rpc(&mut fx.proxy, &fx.rx, json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}
    }));
    let tools: Vec<String> = resp["result"]["tools"]
        .as_array().unwrap()
        .iter().map(|t| t["name"].as_str().unwrap().to_string())
        .collect();

    for expected in ["workflow_execute","workflow_status","workflow_events","workflow_cancel",
                     "workflow_list_files","workflow_list_runs","workflow_validate_schema"] {
        assert!(tools.contains(&expected.to_string()), "missing '{expected}'");
    }
}

#[tokio::test]
#[serial]
async fn e2e_execute_and_status() {
    let mut fx = McpFixture::spawn("success").await;
    mcp_init(&mut fx.proxy, &fx.rx);

    let script = r#"
        meta = { reasoning = "e2e", phases = { { label = "work" } } }
        function main()
            phase("work")
            local r = agent({ prompt = "do work", model = "mock" })
            report({ ok = r.ok })
        end
    "#;
    let result = call_tool(&mut fx.proxy, &fx.rx, 2, "workflow_execute", json!({
        "script": script, "concurrency": 1, "backend": "mock"
    }));
    let run_id = tool_json(&result)["run_id"].as_str().unwrap().to_string();

    let status = poll_until_done(&mut fx.proxy, &fx.rx, &run_id);
    assert_eq!(status["status"], "completed");
    assert!(status["completed_agents"].as_u64().unwrap_or(0) > 0);
}

#[tokio::test]
#[serial]
async fn e2e_execute_and_events() {
    let mut fx = McpFixture::spawn("success").await;
    mcp_init(&mut fx.proxy, &fx.rx);

    let script = r#"
        meta = { reasoning = "events e2e", phases = { { label = "work" } } }
        function main()
            phase("work")
            local r = agent({ prompt = "work", model = "mock" })
            report({ ok = r.ok })
        end
    "#;
    let result = call_tool(&mut fx.proxy, &fx.rx, 2, "workflow_execute", json!({
        "script": script, "concurrency": 1, "backend": "mock"
    }));
    let run_id = tool_json(&result)["run_id"].as_str().unwrap().to_string();
    poll_until_done(&mut fx.proxy, &fx.rx, &run_id);

    let result = call_tool(&mut fx.proxy, &fx.rx, 3, "workflow_events", json!({"run_id": &run_id}));
    let events = tool_json(&result);
    assert!(events["total_matching"].as_u64().unwrap_or(0) > 0);
    assert!(!events["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn e2e_execute_and_list_runs() {
    let mut fx = McpFixture::spawn("success").await;
    mcp_init(&mut fx.proxy, &fx.rx);

    let script = "meta = { reasoning = \"list\", phases = {} }\nfunction main()\n  report({ done = true })\nend";
    let result = call_tool(&mut fx.proxy, &fx.rx, 2, "workflow_execute", json!({
        "script": script, "concurrency": 1, "backend": "mock"
    }));
    let run_id = tool_json(&result)["run_id"].as_str().unwrap().to_string();

    let status = poll_until_done(&mut fx.proxy, &fx.rx, &run_id);
    let uuid = status["run_id"].as_str().unwrap().to_string();

    let result = call_tool(&mut fx.proxy, &fx.rx, 3, "workflow_list_runs", json!({}));
    let list = tool_json(&result);
    assert!(list["count"].as_u64().unwrap_or(0) > 0);
    assert!(list["runs"].as_array().unwrap().iter().any(|r| r["run_id"] == uuid),
        "run {uuid} not in list_runs");
}

#[tokio::test]
#[serial]
async fn e2e_invalid_script_error() {
    let mut fx = McpFixture::spawn("success").await;
    mcp_init(&mut fx.proxy, &fx.rx);

    let resp = rpc(&mut fx.proxy, &fx.rx, json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "workflow_execute",
                   "arguments": {"script": "not valid lua !!!", "backend": "mock"}}
    }));
    assert!(resp.get("error").is_some() || resp["result"]["isError"].as_bool().unwrap_or(false),
        "expected error, got: {resp}");
}

#[tokio::test]
#[serial]
async fn e2e_nonexistent_run_id_error() {
    let mut fx = McpFixture::spawn("success").await;
    mcp_init(&mut fx.proxy, &fx.rx);

    let resp = rpc(&mut fx.proxy, &fx.rx, json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "workflow_status", "arguments": {"run_id": "does-not-exist-12345"}}
    }));
    assert!(resp.get("error").is_some() || resp["result"]["isError"].as_bool().unwrap_or(false),
        "expected error, got: {resp}");
}

#[tokio::test]
#[serial]
async fn e2e_execute_and_cancel() {
    let mut fx = McpFixture::spawn("hang").await;
    mcp_init(&mut fx.proxy, &fx.rx);

    let script = r#"
        meta = { reasoning = "cancel e2e", phases = { { label = "work" } } }
        function main()
            phase("work", 1)
            local r = agent({ prompt = "hang", model = "mock" })
            report(r)
        end
    "#;
    let result = call_tool(&mut fx.proxy, &fx.rx, 2, "workflow_execute", json!({
        "script": script, "concurrency": 1, "backend": "mock"
    }));
    let run_id = tool_json(&result)["run_id"].as_str().unwrap().to_string();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if tokio::time::Instant::now() >= deadline { panic!("run not observable"); }
        let r = call_tool(&mut fx.proxy, &fx.rx, 3, "workflow_status", json!({"run_id": &run_id}));
        if tool_json(&r)["status"] == "running" { break; }
        std::thread::sleep(Duration::from_millis(100));
    }

    let result = call_tool(&mut fx.proxy, &fx.rx, 4, "workflow_cancel", json!({"run_id": &run_id}));
    assert_eq!(tool_json(&result)["result"], "cancelling");

    let status = poll_until_done(&mut fx.proxy, &fx.rx, &run_id);
    assert_eq!(status["status"], "cancelled");
}

#[tokio::test]
#[serial]
async fn e2e_daemon_reconnect() {
    let mut fx = McpFixture::spawn("success").await;
    mcp_init(&mut fx.proxy, &fx.rx);

    let _ = call_tool(&mut fx.proxy, &fx.rx, 2, "workflow_list_files", json!({}));

    // Kill daemon — proxy will detect WS disconnect and retry connecting.
    // The proxy's run_proxy doesn't autostart on reconnect (only at initial
    // startup), so we restart the daemon to simulate an external restart.
    fx.kill_daemon();
    std::thread::sleep(Duration::from_secs(2));

    // Restart daemon on the same port + LUFT_HOME so proxy can find it
    let port_str = TEST_PORT.to_string();
    let common_env: Vec<(&str, &str)> = vec![
        ("LUFT_HOME", fx.home_dir.path().to_str().unwrap()),
        ("LUFT_MOCK_BEHAVIOR", "success"),
        ("LUFT_DAEMON_PORT", &port_str),
    ];
    let _new_daemon = Command::new(BINARY)
        .args(["daemon", "start", "--foreground", "--port", &port_str])
        .env_clear()
        .envs(env_keep())
        .envs(common_env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("restart daemon");

    // Wait for proxy to reconnect + replay handshake.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if tokio::time::Instant::now() >= deadline {
            panic!("proxy did not recover within 30s after daemon restart");
        }
        let resp = rpc(&mut fx.proxy, &fx.rx, json!({
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": {"name": "workflow_list_files", "arguments": {}}
        }));
        if resp.get("result").is_some() {
            let files = tool_json(&resp["result"]);
            assert!(files.is_array(), "expected array after reconnect: {files}");
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
