//! `mcp` — Luft MCP data-plane server (M4).
//!
//! This module implements the MCP server that agents connect to for structured reporting.
//! It provides tools for agents to report findings, artifacts, and logs back to Luft.
//!
//! MCP Tools provided:
//! - `report_finding` — Report a structured finding with severity, kind, title, detail
//! - `report_artifacts` — Report generated artifacts (files, inline content)
//! - `report_log` — Report a log message
//! - `report_status` — Report agent status (progress, completion)
//! - `request_next_task` — Request the next task from the queue (for converge)

use luft_core::contract::finding::{Finding, Severity};
use luft_core::contract::ids::{AgentId, RunId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// MCP endpoint configuration injected into agents.
#[derive(Debug, Clone)]
pub struct McpEndpointConfig {
    pub name: String,
    pub url: String,
    pub run_id: RunId,
    pub agent_id: AgentId,
    pub auth_token: Option<String>,
}

/// Shared state for MCP findings and artifacts.
#[derive(Debug, Default)]
pub struct McpStore {
    pub findings: RwLock<Vec<Finding>>,
    pub artifacts: RwLock<Vec<ArtifactReport>>,
    pub logs: RwLock<Vec<LogReport>>,
    pub statuses: RwLock<HashMap<AgentId, StatusReport>>,
}

impl McpStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Add a finding to the store.
    pub fn add_finding(&self, finding: Finding) {
        let mut findings = self.findings.write().unwrap();
        findings.push(finding);
    }

    /// Get all findings for a run.
    pub fn get_findings(&self) -> Vec<Finding> {
        self.findings.read().unwrap().clone()
    }

    /// Clear findings for a specific run.
    pub fn clear_findings(&self) {
        let mut findings = self.findings.write().unwrap();
        findings.clear();
    }

    /// Add an artifact.
    pub fn add_artifact(&self, artifact: ArtifactReport) {
        let mut artifacts = self.artifacts.write().unwrap();
        artifacts.push(artifact);
    }

    /// Get all artifacts.
    pub fn get_artifacts(&self) -> Vec<ArtifactReport> {
        self.artifacts.read().unwrap().clone()
    }

    /// Get all logs.
    pub fn get_logs(&self) -> Vec<LogReport> {
        self.logs.read().unwrap().clone()
    }

    /// Add a log entry.
    pub fn add_log(&self, log: LogReport) {
        let mut logs = self.logs.write().unwrap();
        logs.push(log);
    }

    /// Update agent status.
    pub fn update_status(&self, agent_id: AgentId, status: StatusReport) {
        let mut statuses = self.statuses.write().unwrap();
        statuses.insert(agent_id, status);
    }

    /// Get status for an agent.
    pub fn get_status(&self, agent_id: &AgentId) -> Option<StatusReport> {
        let statuses = self.statuses.read().unwrap();
        statuses.get(agent_id).cloned()
    }
}

/// An artifact reported by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactReport {
    pub key: String,
    pub path: Option<String>,
    pub inline: Option<serde_json::Value>,
    pub agent_id: AgentId,
    pub ts: u64,
}

/// A log entry reported by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogReport {
    pub level: String,
    pub msg: String,
    pub agent_id: AgentId,
    pub ts: u64,
}

/// Agent status report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    pub status: String,
    pub progress: Option<f32>,
    pub message: Option<String>,
    pub agent_id: AgentId,
    pub ts: u64,
}

// ============================================================================
// MCP Protocol Implementation
// ============================================================================

/// MCP JSON-RPC request.
#[derive(Debug, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum McpRequest {
    #[serde(rename = "initialize")]
    Initialize {
        protocol_version: String,
        capabilities: ClientCapabilities,
    },
    #[serde(rename = "tools/list")]
    ToolsList,
    #[serde(rename = "tools/call")]
    ToolsCall {
        name: String,
        arguments: serde_json::Value,
    },
    #[serde(rename = "notifications/initialized")]
    NotificationsInitialized,
    #[serde(rename = "ping")]
    Ping,
}

/// MCP client capabilities.
#[derive(Debug, Deserialize, Default)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub tools: Option<ToolsCapability>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ToolsCapability {
    #[serde(default)]
    pub list_changed: Option<bool>,
}

/// MCP JSON-RPC response.
#[derive(Debug, Serialize)]
pub struct McpResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpError>,
}

#[derive(Debug, Serialize)]
pub struct McpError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl McpResponse {
    pub fn result(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<serde_json::Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(McpError {
                code,
                message,
                data: None,
            }),
        }
    }

    pub fn notification(method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id: None,
            result: Some(serde_json::json!({
                "method": method,
                "params": params
            })),
            error: None,
        }
    }
}

/// MCP tool definitions.
pub fn get_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "report_finding".to_string(),
            description: "Report a structured finding from analysis".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "description": "Category of finding (e.g., 'missing_auth', 'source', 'bug', 'security')"
                    },
                    "severity": {
                        "type": "string",
                        "enum": ["info", "low", "medium", "high", "critical"],
                        "description": "Severity level of the finding"
                    },
                    "title": {
                        "type": "string",
                        "description": "Short title describing the finding"
                    },
                    "detail": {
                        "type": "string",
                        "description": "Detailed description of the finding"
                    },
                    "location": {
                        "type": "object",
                        "properties": {
                            "file": { "type": "string" },
                            "line": { "type": "number" }
                        },
                        "description": "Optional file:line location"
                    },
                    "evidence": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Supporting evidence or citations"
                    },
                    "data": {
                        "type": "object",
                        "description": "Additional structured data"
                    }
                },
                "required": ["kind", "severity", "title", "detail"]
            }),
        },
        ToolDefinition {
            name: "report_artifacts".to_string(),
            description: "Report generated artifacts from the agent".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "artifacts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "key": { "type": "string" },
                                "path": { "type": "string" },
                                "inline": { "type": "object" }
                            },
                            "required": ["key"]
                        }
                    }
                },
                "required": ["artifacts"]
            }),
        },
        ToolDefinition {
            name: "report_log".to_string(),
            description: "Report a log message".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "level": {
                        "type": "string",
                        "enum": ["trace", "debug", "info", "warn", "error"],
                        "description": "Log level"
                    },
                    "msg": {
                        "type": "string",
                        "description": "Log message"
                    }
                },
                "required": ["level", "msg"]
            }),
        },
        ToolDefinition {
            name: "report_status".to_string(),
            description: "Report agent status and progress".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["started", "progress", "completed", "failed"],
                        "description": "Current status"
                    },
                    "progress": {
                        "type": "number",
                        "description": "Progress percentage (0.0-1.0)"
                    },
                    "message": {
                        "type": "string",
                        "description": "Status message"
                    }
                },
                "required": ["status"]
            }),
        },
        ToolDefinition {
            name: "request_next_task".to_string(),
            description: "Request the next task from the converge queue".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
    ]
}

#[derive(Debug, Serialize, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// Run the MCP server on stdio (for Claude Code compatibility).
pub async fn run_mcp_server(
    store: Arc<McpStore>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let mut reader = BufReader::new(stdin).lines();
    let mut writer = stdout;

    let mut initialized = false;

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let response = McpResponse::error(None, -32700, format!("Parse error: {}", e));
                let json = serde_json::to_string(&response)?;
                writer.write_all(json.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                continue;
            }
        };

        let response = handle_request(&request, &store, &mut initialized).await;
        let json = serde_json::to_string(&response)?;
        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

async fn handle_request(
    request: &serde_json::Value,
    store: &Arc<McpStore>,
    initialized: &mut bool,
) -> McpResponse {
    let id = request.get("id").cloned();
    let method = match request.get("method").and_then(|v| v.as_str()) {
        Some(m) => m,
        None => {
            return McpResponse::error(id, -32600, "Missing method".to_string());
        }
    };

    let params = request.get("params");

    match method {
        "initialize" => {
            *initialized = true;
            McpResponse::result(
                id,
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": ServerCapabilities {
                        tools: ToolsServerCapability {
                            list_changed: false
                        }
                    },
                    "serverInfo": {
                        "name": "luft",
                        "version": "0.1.0"
                    }
                }),
            )
        }

        "tools/list" => {
            let tools = get_tool_definitions();
            McpResponse::result(
                id,
                serde_json::json!({
                    "tools": tools
                }),
            )
        }

        "tools/call" => {
            let name = params
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = params
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default()));

            handle_tool_call(name, arguments, store, id).await
        }

        "ping" => McpResponse::result(id, serde_json::json!({})),

        "notifications/initialized" => McpResponse::result(id, serde_json::json!({})),

        _ => McpResponse::error(id, -32601, format!("Unknown method: {}", method)),
    }
}

async fn handle_tool_call(
    name: &str,
    arguments: serde_json::Value,
    store: &Arc<McpStore>,
    id: Option<serde_json::Value>,
) -> McpResponse {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    match name {
        "report_finding" => {
            let kind = arguments
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let severity_str = arguments
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("info");
            let severity = match severity_str {
                "critical" => Severity::Critical,
                "high" => Severity::High,
                "medium" => Severity::Medium,
                "low" => Severity::Low,
                _ => Severity::Info,
            };
            let title = arguments
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let detail = arguments
                .get("detail")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let location =
                arguments
                    .get("location")
                    .map(|loc| luft_core::contract::finding::Location {
                        file: std::path::PathBuf::from(
                            loc.get("file").and_then(|v| v.as_str()).unwrap_or(""),
                        ),
                        line: loc.get("line").and_then(|v| v.as_u64()).map(|l| l as u32),
                    });

            let evidence: Vec<String> = arguments
                .get("evidence")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let data = arguments
                .get("data")
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            let finding = Finding {
                kind,
                severity,
                title,
                detail,
                location,
                evidence,
                data,
            };

            store.add_finding(finding);

            McpResponse::result(
                id,
                serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": "Finding reported successfully"
                    }]
                }),
            )
        }

        "report_artifacts" => {
            let artifacts: Vec<ArtifactReport> = arguments
                .get("artifacts")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| {
                            let key = v.get("key")?.as_str()?.to_string();
                            let path = v.get("path").and_then(|p| p.as_str()).map(String::from);
                            let inline = v.get("inline").cloned();
                            Some(ArtifactReport {
                                key,
                                path,
                                inline,
                                agent_id: uuid::Uuid::nil(),
                                ts,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            for artifact in &artifacts {
                store.add_artifact(artifact.clone());
            }

            McpResponse::result(
                id,
                serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": format!("{} artifact(s) reported", artifacts.len())
                    }]
                }),
            )
        }

        "report_log" => {
            let level = arguments
                .get("level")
                .and_then(|v| v.as_str())
                .unwrap_or("info")
                .to_string();
            let msg = arguments
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            store.add_log(LogReport {
                level,
                msg,
                agent_id: uuid::Uuid::nil(),
                ts,
            });

            McpResponse::result(
                id,
                serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": "Log recorded"
                    }]
                }),
            )
        }

        "report_status" => {
            let status = arguments
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("progress")
                .to_string();
            let progress = arguments
                .get("progress")
                .and_then(|v| v.as_f64())
                .map(|p| p as f32);
            let message = arguments
                .get("message")
                .and_then(|v| v.as_str())
                .map(String::from);

            store.update_status(
                uuid::Uuid::nil(),
                StatusReport {
                    status,
                    progress,
                    message,
                    agent_id: uuid::Uuid::nil(),
                    ts,
                },
            );

            McpResponse::result(
                id,
                serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": "Status updated"
                    }]
                }),
            )
        }

        "request_next_task" => {
            // This is handled by the converge logic in the runtime
            McpResponse::result(
                id,
                serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": "Task queue is empty"
                    }],
                    "isError": true
                }),
            )
        }

        _ => McpResponse::error(id, -32602, format!("Unknown tool: {}", name)),
    }
}

#[derive(Debug, Serialize)]
pub struct ServerCapabilities {
    pub tools: ToolsServerCapability,
}

#[derive(Debug, Serialize)]
pub struct ToolsServerCapability {
    #[serde(rename = "listChanged")]
    pub list_changed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_finding_to_store() {
        let store = McpStore::new();
        let finding = Finding {
            kind: "test".to_string(),
            severity: Severity::High,
            title: "Test Finding".to_string(),
            detail: "Test detail".to_string(),
            location: None,
            evidence: vec![],
            data: serde_json::Value::Null,
        };

        store.add_finding(finding);
        let findings = store.get_findings();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "Test Finding");
    }

    #[test]
    fn test_tool_definitions() {
        let tools = get_tool_definitions();
        assert!(!tools.is_empty());
        let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"report_finding"));
        assert!(names.contains(&"report_artifacts"));
        assert!(names.contains(&"report_log"));
        assert!(names.contains(&"report_status"));
    }

    #[test]
    fn test_mcp_store_new_creates_empty_store() {
        let store = McpStore::new();
        let findings = store.get_findings();
        assert!(findings.is_empty());

        let artifacts = store.get_artifacts();
        assert!(artifacts.is_empty());

        let logs = store.get_logs();
        assert!(logs.is_empty());
    }

    #[test]
    fn test_mcp_store_add_and_get_findings() {
        let store = McpStore::new();
        let finding1 = Finding {
            kind: "test1".to_string(),
            severity: Severity::Info,
            title: "Test Finding 1".to_string(),
            detail: "Test detail 1".to_string(),
            location: None,
            evidence: vec![],
            data: serde_json::Value::Null,
        };

        let finding2 = Finding {
            kind: "test2".to_string(),
            severity: Severity::High,
            title: "Test Finding 2".to_string(),
            detail: "Test detail 2".to_string(),
            location: None,
            evidence: vec![],
            data: serde_json::Value::Null,
        };

        store.add_finding(finding1);
        store.add_finding(finding2);

        let findings = store.get_findings();
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].title, "Test Finding 1");
        assert_eq!(findings[1].title, "Test Finding 2");
    }

    #[test]
    fn test_mcp_store_clear_findings() {
        let store = McpStore::new();
        let finding = Finding {
            kind: "test".to_string(),
            severity: Severity::Info,
            title: "Test Finding".to_string(),
            detail: "Test detail".to_string(),
            location: None,
            evidence: vec![],
            data: serde_json::Value::Null,
        };

        store.add_finding(finding.clone());
        assert_eq!(store.get_findings().len(), 1);

        store.clear_findings();
        assert!(store.get_findings().is_empty());
    }

    #[test]
    fn test_mcp_store_add_and_get_artifacts() {
        let store = McpStore::new();
        let artifact = ArtifactReport {
            key: "test_artifact".to_string(),
            path: Some("/path/to/file.txt".to_string()),
            inline: Some(serde_json::json!({"content": "test content"})),
            agent_id: uuid::Uuid::now_v7(),
            ts: 12345,
        };

        store.add_artifact(artifact);
        let artifacts = store.get_artifacts();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].key, "test_artifact");
        assert_eq!(artifacts[0].path, Some("/path/to/file.txt".to_string()));
    }

    #[test]
    fn test_mcp_store_add_and_get_logs() {
        let store = McpStore::new();
        let log = LogReport {
            level: "info".to_string(),
            msg: "Test log message".to_string(),
            agent_id: uuid::Uuid::now_v7(),
            ts: 12345,
        };

        store.add_log(log.clone());
        store.add_log(log);

        let logs = store.get_logs();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].msg, "Test log message");
        assert_eq!(logs[0].level, "info");
    }

    #[test]
    fn test_mcp_store_update_and_get_status() {
        let store = McpStore::new();
        let agent_id = uuid::Uuid::now_v7();
        let status = StatusReport {
            status: "running".to_string(),
            progress: Some(0.5),
            message: Some("Processing...".to_string()),
            agent_id,
            ts: 12345,
        };

        store.update_status(agent_id, status.clone());
        let retrieved_status = store.get_status(&agent_id);

        assert!(retrieved_status.is_some());
        let retrieved = retrieved_status.unwrap();
        assert_eq!(retrieved.status, "running");
        assert_eq!(retrieved.progress, Some(0.5));
        assert_eq!(retrieved.message, Some("Processing...".to_string()));
    }

    #[test]
    fn test_mcp_store_get_nonexistent_status() {
        let store = McpStore::new();
        let agent_id = uuid::Uuid::now_v7();
        let status = store.get_status(&agent_id);
        assert!(status.is_none());
    }

    #[tokio::test]
    async fn test_handle_tool_call_unknown_tool() {
        let store = McpStore::new();
        let unknown_tool_name = "unknown_tool";
        let arguments = serde_json::json!({});

        let response = handle_tool_call(unknown_tool_name, arguments, &store, None).await;

        assert!(response.result.is_none());
        assert!(response.error.is_some());
        if let Some(error) = response.error {
            assert_eq!(error.code, -32602);
            assert!(error.message.contains("Unknown tool"));
        }
    }

    #[tokio::test]
    async fn test_handle_tool_call_report_finding() {
        let store = McpStore::new();
        let arguments = serde_json::json!({
            "kind": "test",
            "severity": "high",
            "title": "Test Finding",
            "detail": "Test detail",
            "location": {
                "file": "test.rs",
                "line": 42
            },
            "evidence": ["evidence1", "evidence2"],
            "data": {"key": "value"}
        });

        let response = handle_tool_call("report_finding", arguments, &store, None).await;

        assert!(response.result.is_some());
        assert!(response.error.is_none());

        let findings = store.get_findings();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "test");
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[0].title, "Test Finding");
        assert_eq!(findings[0].evidence.len(), 2);
    }

    #[tokio::test]
    async fn test_handle_tool_call_report_artifacts() {
        let store = McpStore::new();
        let arguments = serde_json::json!({
            "artifacts": [
                {
                    "key": "artifact1",
                    "path": "/path/file1.txt"
                },
                {
                    "key": "artifact2",
                    "inline": {"content": "inline content"}
                }
            ]
        });

        let response = handle_tool_call("report_artifacts", arguments, &store, None).await;

        assert!(response.result.is_some());
        assert!(response.error.is_none());

        let artifacts = store.get_artifacts();
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0].key, "artifact1");
        assert_eq!(artifacts[1].key, "artifact2");
    }

    #[tokio::test]
    async fn test_handle_tool_call_report_log() {
        let store = McpStore::new();
        let arguments = serde_json::json!({
            "level": "error",
            "msg": "Error occurred"
        });

        let response = handle_tool_call("report_log", arguments, &store, None).await;

        assert!(response.result.is_some());
        assert!(response.error.is_none());

        let logs = store.get_logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].level, "error");
        assert_eq!(logs[0].msg, "Error occurred");
    }

    #[tokio::test]
    async fn test_handle_tool_call_report_status() {
        let store = McpStore::new();
        let _agent_id = uuid::Uuid::now_v7();
        let arguments = serde_json::json!({
            "status": "progress",
            "progress": 0.75,
            "message": "75% complete"
        });

        let response = handle_tool_call("report_status", arguments, &store, None).await;

        assert!(response.result.is_some());
        assert!(response.error.is_none());

        let status = store.get_status(&uuid::Uuid::nil()); // The function uses uuid::Uuid::nil()
        assert!(status.is_some());
        let retrieved = status.unwrap();
        assert_eq!(retrieved.status, "progress");
        assert_eq!(retrieved.progress, Some(0.75));
        assert_eq!(retrieved.message, Some("75% complete".to_string()));
    }

    #[test]
    fn test_mcp_response_result() {
        let result_value = serde_json::json!({"success": true});
        let response = McpResponse::result(Some(serde_json::json!(1)), result_value.clone());

        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.id, Some(serde_json::json!(1)));
        assert_eq!(response.result, Some(result_value));
        assert!(response.error.is_none());
    }

    #[test]
    fn test_mcp_response_error() {
        let response =
            McpResponse::error(Some(serde_json::json!(2)), -32600, "Test error".to_string());

        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.id, Some(serde_json::json!(2)));
        assert!(response.result.is_none());
        assert!(response.error.is_some());
        if let Some(error) = response.error {
            assert_eq!(error.code, -32600);
            assert_eq!(error.message, "Test error");
        }
    }

    #[test]
    fn test_mcp_response_notification() {
        let params = serde_json::json!({"key": "value"});
        let response = McpResponse::notification("test_method", params);

        assert_eq!(response.jsonrpc, "2.0");
        assert!(response.id.is_none());
        assert!(response.error.is_none());
        assert!(response.result.is_some());
    }

    #[test]
    fn test_mcp_store_empty_list_tools() {
        let _store = McpStore::new();
        let tools = get_tool_definitions();
        assert!(!tools.is_empty());
        assert_eq!(tools.len(), 5); // Should have 5 predefined tools
    }

    #[test]
    fn test_add_finding_and_get_it() {
        let store = McpStore::new();
        let finding = Finding {
            kind: "security".to_string(),
            severity: Severity::Critical,
            title: "Security Finding".to_string(),
            detail: "Critical security issue found".to_string(),
            location: Some(luft_core::contract::finding::Location {
                file: std::path::PathBuf::from("src/main.rs"),
                line: Some(42),
            }),
            evidence: vec!["Line 42 has vulnerability".to_string()],
            data: serde_json::json!({"cve": "CVE-2024-1234"}),
        };

        store.add_finding(finding.clone());
        let findings = store.get_findings();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "security");
        assert_eq!(findings[0].severity, Severity::Critical);
    }

    #[tokio::test]
    async fn test_handle_tool_call_request_next_task() {
        let store = McpStore::new();
        let arguments = serde_json::json!({});

        let response = handle_tool_call("request_next_task", arguments, &store, None).await;

        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }
}
