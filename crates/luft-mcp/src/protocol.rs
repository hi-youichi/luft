//! JSON-RPC 2.0 protocol types for the MCP server.
//!
//! These types cover the subset of [MCP] messages the server handles:
//! `initialize`, `ping`, `resources/list`, `resources/templates/list`,
//! `resources/read`, `tools/list`, `tools/call`, and the
//! `notifications/initialized` notification.
//!
//! [MCP]: https://spec.modelcontextprotocol.io/

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── JSON-RPC envelope ───────────────────────────────────────────────────

/// An incoming JSON-RPC request or notification.
///
/// `id` is `None` for notifications (fire-and-forget), `Some` for requests
/// that expect a response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcMessage {
    /// Always `"2.0"`.
    #[serde(default = "default_version")]
    pub jsonrpc: String,
    /// Request id (number or string). `None` for notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Method name (e.g. `"initialize"`, `"tools/call"`).
    pub method: Option<String>,
    /// Method parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

fn default_version() -> String {
    "2.0".to_string()
}

/// A successful JSON-RPC response.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    pub result: Value,
}

/// A JSON-RPC error response.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub jsonrpc: String,
    pub id: Value,
    pub error: RpcError,
}

/// The `error` object inside a JSON-RPC error response.
#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    /// Create a success response for the given id.
    pub fn new(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result,
        }
    }

    /// Serialize to a single JSON line (for stdio transport).
    pub fn to_line(&self) -> serde_json::Result<String> {
        let mut s = serde_json::to_string(self)?;
        s.push('\n');
        Ok(s)
    }
}

impl JsonRpcError {
    /// Create an error response for the given id, code, and message.
    pub fn new(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            error: RpcError {
                code,
                message: message.into(),
            },
        }
    }

    /// Serialize to a single JSON line (for stdio transport).
    pub fn to_line(&self) -> serde_json::Result<String> {
        let mut s = serde_json::to_string(self)?;
        s.push('\n');
        Ok(s)
    }
}

// ── Standard error codes ────────────────────────────────────────────────

/// JSON-RPC standard error codes.
pub mod error_codes {
    /// Method does not exist.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Invalid method parameters.
    pub const INVALID_PARAMS: i32 = -32602;
    /// Internal server error.
    pub const INTERNAL_ERROR: i32 = -32603;
    /// Parse error.
    pub const PARSE_ERROR: i32 = -32700;
}

// ── MCP-specific result builders ────────────────────────────────────────

/// The result object returned by `initialize`.
pub fn initialize_result() -> Value {
    serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {},
            "resources": {}
        },
        "serverInfo": {
            "name": "luft",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

/// The `resources/list` result — static resources only (schema + examples list).
pub fn resources_list_result() -> Value {
    serde_json::json!({
        "resources": [
            {
                "uri": "workflow://schema",
                "name": "Workflow DSL Reference",
                "mimeType": "text/markdown",
                "description": "Complete Lua DSL syntax for writing Luft workflows"
            },
            {
                "uri": "workflow://examples",
                "name": "Example Workflows",
                "mimeType": "application/json",
                "description": "List of available example workflows"
            }
        ]
    })
}

/// The `resources/templates/list` result — the dynamic `workflow://example/{name}` template.
pub fn resource_templates_list_result() -> Value {
    serde_json::json!({
        "resourceTemplates": [
            {
                "uriTemplate": "workflow://example/{name}",
                "name": "Example Workflow",
                "description": "Read a specific example workflow by name",
                "mimeType": "text/x-lua"
            }
        ]
    })
}

/// The `tools/list` result — four tools: execute_workflow, list_workflows,
/// get_run_status, get_run_events.
pub fn tools_list_result() -> Value {
    serde_json::json!({
        "tools": [
            {
                "name": "execute_workflow",
                "description": "Execute a Luft workflow. Accepts either inline Lua script or a path to a .lua file. Returns immediately with a run_id — use get_run_status to poll progress.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Inline Lua workflow script" },
                        "path": { "type": "string", "description": "Path to .lua file (relative to CWD)" },
                        "args": { "type": "object", "description": "Workflow arguments, accessible as `args` in Lua" }
                    }
                }
            },
            {
                "name": "list_workflows",
                "description": "List available workflow files from workflows/ and examples/ directories",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "get_run_status",
                "description": "Get the current status of a workflow run",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "run_id": { "type": "string" }
                    },
                    "required": ["run_id"]
                }
            },
            {
                "name": "get_run_events",
                "description": "Get events for a workflow run, optionally only those after a specific event ID",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "run_id": { "type": "string" },
                        "since_event_id": { "type": "string", "description": "Only return events after this event ID (for incremental polling)" }
                    },
                    "required": ["run_id"]
                }
            }
        ]
    })
}

/// Build an MCP tool-call result (text content + optional error flag).
pub fn tool_result(text: &str, is_error: bool) -> Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── JsonRpcMessage deserialization ──────────────────────────────────

    #[test]
    fn parse_full_request() {
        let msg: JsonRpcMessage =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
                .unwrap();
        assert_eq!(msg.method.as_deref(), Some("initialize"));
        assert_eq!(msg.id, Some(Value::from(1)));
        assert!(msg.params.is_some());
    }

    #[test]
    fn parse_notification_no_id() {
        let msg: JsonRpcMessage =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .unwrap();
        assert_eq!(msg.method.as_deref(), Some("notifications/initialized"));
        assert!(msg.id.is_none());
    }

    #[test]
    fn parse_string_id() {
        let msg: JsonRpcMessage =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":"req-abc","method":"ping"}"#).unwrap();
        assert_eq!(msg.id, Some(Value::from("req-abc")));
    }

    #[test]
    fn parse_without_jsonrpc_defaults_to_2_0() {
        let msg: JsonRpcMessage = serde_json::from_str(r#"{"id":1,"method":"ping"}"#).unwrap();
        assert_eq!(msg.jsonrpc, "2.0");
    }

    // ── JsonRpcResponse ─────────────────────────────────────────────────

    #[test]
    fn response_new_and_serialize() {
        let r = JsonRpcResponse::new(Value::from(42), serde_json::json!({"ok": true}));
        let line = r.to_line().unwrap();
        let parsed: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 42);
        assert_eq!(parsed["result"]["ok"], true);
        assert!(line.ends_with('\n'));
    }

    // ── JsonRpcError ────────────────────────────────────────────────────

    #[test]
    fn error_new_and_serialize() {
        let e = JsonRpcError::new(Value::from(7), -32601, "Method not found");
        let line = e.to_line().unwrap();
        let parsed: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 7);
        assert_eq!(parsed["error"]["code"], -32601);
        assert_eq!(parsed["error"]["message"], "Method not found");
    }

    // ── Result builders ─────────────────────────────────────────────────

    #[test]
    fn initialize_result_has_capabilities() {
        let r = initialize_result();
        assert_eq!(r["protocolVersion"], "2024-11-05");
        assert!(r["capabilities"]["tools"].is_object());
        assert!(r["capabilities"]["resources"].is_object());
        assert_eq!(r["serverInfo"]["name"], "luft");
    }

    #[test]
    fn resources_list_has_two_static_resources() {
        let r = resources_list_result();
        let resources = r["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0]["uri"], "workflow://schema");
        assert_eq!(resources[1]["uri"], "workflow://examples");
    }

    #[test]
    fn resource_templates_has_example_template() {
        let r = resource_templates_list_result();
        let templates = r["resourceTemplates"].as_array().unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0]["uriTemplate"], "workflow://example/{name}");
    }

    #[test]
    fn tools_list_has_four_tools() {
        let r = tools_list_result();
        let tools = r["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"execute_workflow"));
        assert!(names.contains(&"list_workflows"));
        assert!(names.contains(&"get_run_status"));
        assert!(names.contains(&"get_run_events"));
    }

    #[test]
    fn tool_result_builder() {
        let r = tool_result("hello", false);
        assert_eq!(r["content"][0]["text"], "hello");
        assert_eq!(r["isError"], false);

        let r = tool_result("error msg", true);
        assert_eq!(r["isError"], true);
    }

    #[test]
    fn error_codes_constants() {
        assert_eq!(error_codes::METHOD_NOT_FOUND, -32601);
        assert_eq!(error_codes::INVALID_PARAMS, -32602);
        assert_eq!(error_codes::INTERNAL_ERROR, -32603);
        assert_eq!(error_codes::PARSE_ERROR, -32700);
    }
}
