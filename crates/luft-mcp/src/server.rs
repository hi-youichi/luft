//! Stdio server: JSON-RPC main loop + method dispatch.
//!
//! The server reads newline-delimited JSON-RPC messages from stdin, dispatches
//! each to the appropriate handler, and writes responses to stdout.
//!
//! For testing, the core dispatch logic is extracted into [`dispatch`] which
//! operates on a single message and returns the optional response — the stdio
//! loop is a thin wrapper around it.

use luft::Luft;
use serde_json::Value;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use crate::protocol::{
    error_codes, initialize_result, resource_templates_list_result, resources_list_result,
    tools_list_result, JsonRpcError, JsonRpcMessage, JsonRpcResponse,
};
use crate::resources::build_read_response;
use crate::tools::{handle_call, new_run_registry, RunRegistry};

/// The MCP server: owns the Luft instance and run registry.
pub struct McpServer {
    luft: Luft,
    runs: RunRegistry,
    search_dirs: Vec<PathBuf>,
}

impl McpServer {
    /// Create a new server with the given Luft instance.
    ///
    /// Search directories default to `["examples", "workflows"]` (relative to CWD).
    pub fn new(luft: Luft) -> Self {
        Self {
            luft,
            runs: new_run_registry(),
            search_dirs: vec![PathBuf::from("examples"), PathBuf::from("workflows")],
        }
    }

    /// Override the search directories for example/workflow discovery.
    pub fn search_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.search_dirs = dirs;
        self
    }

    /// Run the stdio main loop: read lines from stdin, dispatch, write to stdout.
    ///
    /// Blocks until stdin is closed (EOF) or an unrecoverable I/O error occurs.
    pub async fn serve_stdio(self) -> io::Result<()> {
        let stdin = io::stdin();
        let mut stdout = io::stdout();

        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let response = self.dispatch_line(&line).await;
            if let Some(resp) = response {
                stdout.write_all(resp.as_bytes())?;
                stdout.flush()?;
            }
        }

        Ok(())
    }

    /// Dispatch a single JSON-RPC line and return the optional response line.
    ///
    /// Returns `None` for notifications (no response needed).
    pub async fn dispatch_line(&self, line: &str) -> Option<String> {
        // Skip blank/whitespace-only lines.
        if line.trim().is_empty() {
            return None;
        }

        // Parse the JSON-RPC message.
        let msg: JsonRpcMessage = match serde_json::from_str(line) {
            Ok(m) => m,
            Err(e) => {
                // Parse error — respond with id null.
                let err = JsonRpcError::new(Value::Null, error_codes::PARSE_ERROR, e.to_string());
                return Some(err.to_line().unwrap_or_default());
            }
        };

        self.dispatch_message(msg).await
    }

    /// Dispatch a parsed JSON-RPC message.
    ///
    /// Returns `None` for notifications (no id).
    pub async fn dispatch_message(&self, msg: JsonRpcMessage) -> Option<String> {
        // Notifications (no id) get no response.
        let id = msg.id.clone()?;

        let method = msg.method.as_deref().unwrap_or("");
        let params = msg.params.clone().unwrap_or(Value::Null);

        match self.dispatch_method(method, &params).await {
            Ok(result) => {
                let resp = JsonRpcResponse::new(id, result);
                Some(resp.to_line().unwrap_or_default())
            }
            Err((code, message)) => {
                let err = JsonRpcError::new(id, code, message);
                Some(err.to_line().unwrap_or_default())
            }
        }
    }

    /// Dispatch a single method. Returns the result value or an error tuple.
    ///
    /// This is the core routing logic, extracted for testability.
    pub async fn dispatch_method(
        &self,
        method: &str,
        params: &Value,
    ) -> Result<Value, (i32, String)> {
        match method {
            "initialize" => Ok(initialize_result()),

            "ping" => Ok(serde_json::json!({})),

            "resources/list" => Ok(resources_list_result()),

            "resources/templates/list" => Ok(resource_templates_list_result()),

            "resources/read" => {
                let uri = params
                    .get("uri")
                    .and_then(|v| v.as_str())
                    .ok_or((error_codes::INVALID_PARAMS, "missing 'uri' parameter".into()))?;
                match build_read_response(uri, &self.search_dirs) {
                    Ok(v) => Ok(v),
                    Err(e) => Err((error_codes::INTERNAL_ERROR, e.to_string())),
                }
            }

            "tools/list" => Ok(tools_list_result()),

            "tools/call" => Ok(handle_call(params, &self.luft, &self.runs, &self.search_dirs).await),

            "notifications/initialized" => {
                // Notification acknowledged — no result.
                Ok(serde_json::json!({}))
            }

            _ => Err((
                error_codes::METHOD_NOT_FOUND,
                format!("method not found: {method}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn build_server() -> McpServer {
        use std::time::Duration;
        use luft_core::{MockBackend, MockBehavior, TokenUsage};
        let backend = MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!({"result": "ok"}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        );
        let luft = luft::Luft::builder()
            .backend(backend)
            .base_dir(tempfile::TempDir::new().unwrap().keep())
            .build()
            .unwrap();
        McpServer::new(luft)
    }

    // ── dispatch_method ─────────────────────────────────────────────────

    #[tokio::test]
    async fn dispatch_initialize() {
        let server = build_server();
        let result = server.dispatch_method("initialize", &json!({})).await.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "luft");
    }

    #[tokio::test]
    async fn dispatch_ping() {
        let server = build_server();
        let result = server.dispatch_method("ping", &json!({})).await.unwrap();
        assert!(result.is_object());
    }

    #[tokio::test]
    async fn dispatch_resources_list() {
        let server = build_server();
        let result = server.dispatch_method("resources/list", &json!({})).await.unwrap();
        assert_eq!(result["resources"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn dispatch_resources_templates_list() {
        let server = build_server();
        let result = server
            .dispatch_method("resources/templates/list", &json!({}))
            .await
            .unwrap();
        assert_eq!(result["resourceTemplates"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dispatch_resources_read_schema() {
        let server = build_server();
        let result = server
            .dispatch_method("resources/read", &json!({"uri": "workflow://schema"}))
            .await
            .unwrap();
        assert_eq!(result["contents"][0]["uri"], "workflow://schema");
        assert_eq!(result["contents"][0]["mimeType"], "text/markdown");
    }

    #[tokio::test]
    async fn dispatch_resources_read_missing_uri() {
        let server = build_server();
        let err = server.dispatch_method("resources/read", &json!({})).await.unwrap_err();
        assert_eq!(err.0, error_codes::INVALID_PARAMS);
        assert!(err.1.contains("missing 'uri'"));
    }

    #[tokio::test]
    async fn dispatch_resources_read_unknown_uri() {
        let server = build_server();
        let err = server
            .dispatch_method("resources/read", &json!({"uri": "workflow://bogus"}))
            .await
            .unwrap_err();
        assert_eq!(err.0, error_codes::INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn dispatch_tools_list() {
        let server = build_server();
        let result = server.dispatch_method("tools/list", &json!({})).await.unwrap();
        assert_eq!(result["tools"].as_array().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn dispatch_tools_call_list_workflows() {
        let server = build_server();
        let result = server
            .dispatch_method("tools/call", &json!({"name": "list_workflows", "arguments": {}}))
            .await
            .unwrap();
        assert_eq!(result["isError"], false);
    }

    #[tokio::test]
    async fn dispatch_unknown_method() {
        let server = build_server();
        let err = server.dispatch_method("bogus/method", &json!({})).await.unwrap_err();
        assert_eq!(err.0, error_codes::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn dispatch_notifications_initialized() {
        let server = build_server();
        let result = server
            .dispatch_method("notifications/initialized", &json!({}))
            .await
            .unwrap();
        assert!(result.is_object());
    }

    // ── dispatch_message (full roundtrip) ───────────────────────────────

    #[tokio::test]
    async fn dispatch_message_initialize() {
        let server = build_server();
        let msg = JsonRpcMessage {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: Some("initialize".into()),
            params: Some(json!({})),
        };
        let resp_line = server.dispatch_message(msg).await.unwrap();
        let parsed: Value = serde_json::from_str(resp_line.trim()).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["result"]["serverInfo"]["name"], "luft");
    }

    #[tokio::test]
    async fn dispatch_message_notification_no_response() {
        let server = build_server();
        let msg = JsonRpcMessage {
            jsonrpc: "2.0".into(),
            id: None,
            method: Some("notifications/initialized".into()),
            params: None,
        };
        assert!(server.dispatch_message(msg).await.is_none());
    }

    #[tokio::test]
    async fn dispatch_message_unknown_method_error() {
        let server = build_server();
        let msg = JsonRpcMessage {
            jsonrpc: "2.0".into(),
            id: Some(json!("req-2")),
            method: Some("unknown".into()),
            params: None,
        };
        let resp_line = server.dispatch_message(msg).await.unwrap();
        let parsed: Value = serde_json::from_str(resp_line.trim()).unwrap();
        assert_eq!(parsed["id"], "req-2");
        assert_eq!(parsed["error"]["code"], -32601);
    }

    // ── dispatch_line (raw JSON string) ─────────────────────────────────

    #[tokio::test]
    async fn dispatch_line_valid_json() {
        let server = build_server();
        let line = r#"{"jsonrpc":"2.0","id":42,"method":"ping"}"#;
        let resp = server.dispatch_line(line).await.unwrap();
        let parsed: Value = serde_json::from_str(resp.trim()).unwrap();
        assert_eq!(parsed["id"], 42);
        assert!(parsed["result"].is_object());
    }

    #[tokio::test]
    async fn dispatch_line_parse_error() {
        let server = build_server();
        let line = "{ invalid json";
        let resp = server.dispatch_line(line).await.unwrap();
        let parsed: Value = serde_json::from_str(resp.trim()).unwrap();
        assert_eq!(parsed["error"]["code"], -32700);
        assert!(parsed["id"].is_null());
    }

    #[tokio::test]
    async fn dispatch_line_empty_skipped() {
        let server = build_server();
        let resp = server.dispatch_line("   ").await;
        assert!(resp.is_none());
    }

    #[tokio::test]
    async fn dispatch_line_notification_no_response() {
        let server = build_server();
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let resp = server.dispatch_line(line).await;
        assert!(resp.is_none());
    }

    // ── McpServer::search_dirs builder ──────────────────────────────────

    #[tokio::test]
    async fn search_dirs_override() {
        use std::time::Duration;
        use luft_core::{MockBackend, MockBehavior, TokenUsage};
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("custom.lua"), "-- custom").unwrap();

        let backend = MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        );
        let luft = luft::Luft::builder()
            .backend(backend)
            .base_dir(tempfile::TempDir::new().unwrap().keep())
            .build()
            .unwrap();
        let server = McpServer::new(luft).search_dirs(vec![dir.path().to_path_buf()]);

        let result = server
            .dispatch_method("tools/call", &json!({"name": "list_workflows", "arguments": {}}))
            .await
            .unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: Vec<Value> = serde_json::from_str(text).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["name"], "custom");
    }

    // ── full protocol interaction: initialize → tools/list → tools/call ─

    #[tokio::test]
    async fn full_protocol_sequence() {
        let server = build_server();

        // 1. initialize
        let resp = server
            .dispatch_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#)
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(resp.trim()).unwrap();
        assert_eq!(parsed["result"]["protocolVersion"], "2024-11-05");

        // 2. notifications/initialized (no response)
        let resp = server
            .dispatch_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await;
        assert!(resp.is_none());

        // 3. tools/list
        let resp = server
            .dispatch_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(resp.trim()).unwrap();
        assert_eq!(parsed["result"]["tools"].as_array().unwrap().len(), 4);

        // 4. tools/call: list_workflows
        let resp = server
            .dispatch_line(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_workflows","arguments":{}}}"#,
            )
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(resp.trim()).unwrap();
        assert_eq!(parsed["result"]["isError"], false);
    }
}
