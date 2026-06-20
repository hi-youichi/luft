//! WebSocket protocol types — client → server and server → client messages.
//!
//! All messages are UTF-8 JSON text frames (max 64 KB). Every client message
//! carries a required `id` field; server responses correlate via `req_id`.

use crate::service::query::StatusOutput;
use crate::core::contract::event::AgentEvent;
use crate::core::contract::finding::Finding;
use crate::core::contract::ids::RunId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Client → Server
// ---------------------------------------------------------------------------

/// Client messages received over WebSocket.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Run {
        id: String,
        payload: RunPayload,
    },
    ConfirmRun {
        id: String,
        payload: ConfirmRunPayload,
    },
    Resume {
        id: String,
        payload: IdPayload,
    },
    Cancel {
        id: String,
        payload: IdPayload,
    },
    Subscribe {
        id: String,
        payload: SubscribePayload,
    },
    Unsubscribe {
        id: String,
        payload: IdPayload,
    },
    GetStatus {
        id: String,
        payload: IdPayload,
    },
    ListRuns {
        id: String,
        payload: ListRunsPayload,
    },
    GetLogs {
        id: String,
        payload: GetLogsPayload,
    },
    GetFindings {
        id: String,
        payload: IdPayload,
    },
    GetReport {
        id: String,
        payload: IdPayload,
    },
    Ping {
        id: String,
    },
}

/// Payload for the `run` message.
#[derive(Debug, Deserialize)]
pub struct RunPayload {
    /// Natural language prompt (planned into Lua by the planner).
    pub nl: Option<String>,
    /// Absolute path to a workflow file on the server's local filesystem.
    pub workflow: Option<PathBuf>,
    /// Inline Lua script string.
    pub script: Option<String>,
    /// Arguments passed to the workflow as a JSON object.
    #[serde(default)]
    pub args: serde_json::Value,
    /// When true, NL runs return a `script_preview` and wait for `confirm_run`.
    #[serde(default)]
    pub confirm: bool,
}

/// Payload for `confirm_run` — approve or reject a pending script preview.
#[derive(Debug, Deserialize)]
pub struct ConfirmRunPayload {
    pub run_id: RunId,
    pub approve: bool,
}

/// Payload for messages that target a single run by ID.
#[derive(Debug, Deserialize)]
pub struct IdPayload {
    pub run_id: RunId,
}

/// Payload for `subscribe` — optionally filter event types.
#[derive(Debug, Deserialize)]
pub struct SubscribePayload {
    pub run_id: RunId,
    /// `None` means all projected events (excluding `acp_raw`).
    pub filter: Option<Vec<String>>,
}

/// Payload for `list_runs` — pagination.
#[derive(Debug, Deserialize)]
pub struct ListRunsPayload {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

/// Payload for `get_logs` — pagination.
#[derive(Debug, Deserialize)]
pub struct GetLogsPayload {
    pub run_id: RunId,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    20
}

// ---------------------------------------------------------------------------
// Server → Client
// ---------------------------------------------------------------------------

/// Server messages sent over WebSocket.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Hello {
        version: &'static str,
        server: &'static str,
        capabilities: Vec<&'static str>,
    },
    Accepted {
        req_id: String,
        run_id: RunId,
    },
    ScriptPreview {
        req_id: String,
        run_id: RunId,
        script: String,
    },
    Event {
        run_id: RunId,
        event: AgentEvent,
    },
    Status {
        req_id: String,
        run_id: RunId,
        data: StatusOutput,
    },
    RunList {
        req_id: String,
        total: usize,
        items: Vec<StatusOutput>,
    },
    Logs {
        req_id: String,
        run_id: RunId,
        total: usize,
        items: Vec<AgentEvent>,
    },
    Findings {
        req_id: String,
        run_id: RunId,
        items: Vec<Finding>,
    },
    Report {
        req_id: String,
        run_id: RunId,
        data: serde_json::Value,
    },
    Ok {
        req_id: String,
    },
    Error {
        req_id: String,
        code: ErrorCode,
        message: String,
    },
    ServerClosing {
        reason: String,
    },
    Pong {
        req_id: String,
    },
}

/// Error codes returned by the server.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    NotFound,
    RunFinished,
    AlreadyRunning,
    BackendError,
    Capacity,
    ConfirmTimeout,
    Internal,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the `type` field name from an `AgentEvent` variant (used for filtering).
pub fn event_type_name(evt: &AgentEvent) -> &'static str {
    match evt {
        AgentEvent::RunStarted { .. } => "run_started",
        AgentEvent::PhaseStarted { .. } => "phase_started",
        AgentEvent::AgentStarted { .. } => "agent_started",
        AgentEvent::AgentProgress { .. } => "agent_progress",
        AgentEvent::AcpRaw { .. } => "acp_raw",
        AgentEvent::AgentDone { .. } => "agent_done",
        AgentEvent::PhaseDone { .. } => "phase_done",
        AgentEvent::RunDone { .. } => "run_done",
        AgentEvent::Log { .. } => "log",
        AgentEvent::BudgetSet { .. } => "budget_set",
        AgentEvent::ReportEmitted { .. } => "report_emitted",
        AgentEvent::ParallelStarted { .. } => "parallel_started",
        AgentEvent::ParallelDone { .. } => "parallel_done",
        AgentEvent::WorkflowStarted { .. } => "workflow_started",
        AgentEvent::WorkflowDone { .. } => "workflow_done",
        AgentEvent::ConvergeStarted { .. } => "converge_started",
        AgentEvent::ConvergeDone { .. } => "converge_done",
        AgentEvent::PipelineStarted { .. } => "pipeline_started",
        AgentEvent::PipelineStageStarted { .. } => "pipeline_stage_started",
        AgentEvent::PipelineItemDone { .. } => "pipeline_item_done",
        AgentEvent::PipelineDone { .. } => "pipeline_done",
    }
}

/// Build the standard capabilities list (for the `hello` message).
pub fn default_capabilities() -> Vec<&'static str> {
    vec![
        "run",
        "confirm_run",
        "resume",
        "cancel",
        "subscribe",
        "unsubscribe",
        "get_status",
        "list_runs",
        "get_logs",
        "get_findings",
        "get_report",
        "script_preview",
        "event_filter",
        "acp_raw",
        "sdk_events",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::backend::AgentStatus;
    use crate::core::contract::event::{LogLevel, ProgressDelta, RunStatus};
    use crate::core::contract::ids::TokenUsage;
    use chrono::Utc;

    #[test]
    fn client_msg_ping_deserialize() {
        let msg: ClientMsg = serde_json::from_str(r#"{"type":"ping","id":"1"}"#).unwrap();
        assert!(matches!(msg, ClientMsg::Ping { id } if id == "1"));
    }

    #[test]
    fn client_msg_run_with_nl() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"run","id":"2","payload":{"nl":"hello"}}"#,
        ).unwrap();
        match msg {
            ClientMsg::Run { id, payload } => {
                assert_eq!(id, "2");
                assert_eq!(payload.nl.as_deref(), Some("hello"));
                assert!(payload.workflow.is_none());
                assert!(payload.script.is_none());
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn client_msg_run_with_script() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"run","id":"3","payload":{"script":"print(1)"}}"#,
        ).unwrap();
        match msg {
            ClientMsg::Run { payload, .. } => {
                assert_eq!(payload.script.as_deref(), Some("print(1)"));
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn client_msg_subscribe_with_filter() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"subscribe","id":"4","payload":{"run_id":"01923456-7890-7abc-def0-123456789abc","filter":["run_started","agent_done"]}}"#,
        ).unwrap();
        match msg {
            ClientMsg::Subscribe { id, payload } => {
                assert_eq!(id, "4");
                assert_eq!(payload.filter.as_ref().unwrap().len(), 2);
            }
            _ => panic!("expected Subscribe"),
        }
    }

    #[test]
    fn client_msg_get_status() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"get_status","id":"5","payload":{"run_id":"01923456-7890-7abc-def0-123456789abc"}}"#,
        ).unwrap();
        assert!(matches!(msg, ClientMsg::GetStatus { id, .. } if id == "5"));
    }

    #[test]
    fn client_msg_list_runs_default_limit() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"list_runs","id":"6","payload":{}}"#,
        ).unwrap();
        match msg {
            ClientMsg::ListRuns { id, payload } => {
                assert_eq!(id, "6");
                assert_eq!(payload.limit, 20);
                assert_eq!(payload.offset, 0);
            }
            _ => panic!("expected ListRuns"),
        }
    }

    #[test]
    fn client_msg_get_logs_with_pagination() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"get_logs","id":"7","payload":{"run_id":"01923456-7890-7abc-def0-123456789abc","limit":50,"offset":10}}"#,
        ).unwrap();
        match msg {
            ClientMsg::GetLogs { id, payload } => {
                assert_eq!(id, "7");
                assert_eq!(payload.limit, 50);
                assert_eq!(payload.offset, 10);
            }
            _ => panic!("expected GetLogs"),
        }
    }

    #[test]
    fn client_msg_confirm_run() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"confirm_run","id":"8","payload":{"run_id":"01923456-7890-7abc-def0-123456789abc","approve":true}}"#,
        ).unwrap();
        assert!(matches!(msg, ClientMsg::ConfirmRun { id, .. } if id == "8"));
    }

    #[test]
    fn client_msg_cancel() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"cancel","id":"9","payload":{"run_id":"01923456-7890-7abc-def0-123456789abc"}}"#,
        ).unwrap();
        assert!(matches!(msg, ClientMsg::Cancel { id, .. } if id == "9"));
    }

    #[test]
    fn client_msg_resume() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"resume","id":"10","payload":{"run_id":"01923456-7890-7abc-def0-123456789abc"}}"#,
        ).unwrap();
        assert!(matches!(msg, ClientMsg::Resume { id, .. } if id == "10"));
    }

    #[test]
    fn client_msg_get_findings() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"get_findings","id":"11","payload":{"run_id":"01923456-7890-7abc-def0-123456789abc"}}"#,
        ).unwrap();
        assert!(matches!(msg, ClientMsg::GetFindings { id, .. } if id == "11"));
    }

    #[test]
    fn client_msg_get_report() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"get_report","id":"12","payload":{"run_id":"01923456-7890-7abc-def0-123456789abc"}}"#,
        ).unwrap();
        assert!(matches!(msg, ClientMsg::GetReport { id, .. } if id == "12"));
    }

    #[test]
    fn client_msg_unsubscribe() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"unsubscribe","id":"13","payload":{"run_id":"01923456-7890-7abc-def0-123456789abc"}}"#,
        ).unwrap();
        assert!(matches!(msg, ClientMsg::Unsubscribe { id, .. } if id == "13"));
    }

    #[test]
    fn client_msg_invalid_type_returns_error() {
        let result = serde_json::from_str::<ClientMsg>(
            r#"{"type":"nonexistent","id":"99"}"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn server_msg_hello_serialize() {
        let msg = ServerMsg::Hello {
            version: "0.1.0",
            server: "maestro",
            capabilities: default_capabilities(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"hello\""));
        assert!(json.contains("\"version\":\"0.1.0\""));
        assert!(json.contains("\"server\":\"maestro\""));
    }

    #[test]
    fn server_msg_pong_serialize() {
        let msg = ServerMsg::Pong { req_id: "1".into() };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"pong\""));
    }

    #[test]
    fn server_msg_error_serialize() {
        let msg = ServerMsg::Error {
            req_id: "2".into(),
            code: ErrorCode::NotFound,
            message: "not found".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"error\""));
        assert!(json.contains("\"code\":\"not_found\""));
    }

    #[test]
    fn event_type_name_all_variants() {
        let run_id = RunId::now_v7();
        let cases: Vec<(AgentEvent, &'static str)> = vec![
            (AgentEvent::RunStarted { run_id, task: "t".into(), ts: Utc::now() }, "run_started"),
            (AgentEvent::PhaseStarted { run_id, phase_id: 0, label: "p".into(), planned: 1 }, "phase_started"),
            (AgentEvent::AgentStarted { run_id, phase_id: 0, agent_id: run_id, prompt_preview: "p".into(), model: None }, "agent_started"),
            (AgentEvent::AgentProgress { run_id, agent_id: run_id, delta: ProgressDelta::Message { text: "d".into() } }, "agent_progress"),
            (AgentEvent::AcpRaw { run_id, agent_id: run_id, kind: "plan".into(), raw: serde_json::json!({"sessionUpdate":"plan"}) }, "acp_raw"),
            (AgentEvent::AgentDone { run_id, agent_id: run_id, status: AgentStatus::Ok, tokens: TokenUsage::default(), elapsed_ms: 0 }, "agent_done"),
            (AgentEvent::PhaseDone { run_id, phase_id: 0, ok: 1, failed: 0 }, "phase_done"),
            (AgentEvent::RunDone { run_id, status: RunStatus::Completed, total_tokens: TokenUsage::default(), report: serde_json::json!(null) }, "run_done"),
            (AgentEvent::Log { run_id, agent_id: None, level: LogLevel::Info, msg: "m".into() }, "log"),
            (AgentEvent::BudgetSet { run_id, time_limit_ms: Some(1), max_rounds: Some(2) }, "budget_set"),
            (AgentEvent::ReportEmitted { run_id, phase_id: 0, report: serde_json::json!({}) }, "report_emitted"),
            (AgentEvent::ParallelStarted { run_id, phase_id: 0, span_id: 1, count: 2 }, "parallel_started"),
            (AgentEvent::ParallelDone { run_id, phase_id: 0, span_id: 1, ok: 2, failed: 0, results: serde_json::json!([]), elapsed_ms: 5 }, "parallel_done"),
            (AgentEvent::WorkflowStarted { run_id, span_id: 1, path: "w.lua".into(), args: serde_json::json!({}) }, "workflow_started"),
            (AgentEvent::WorkflowDone { run_id, span_id: 1, path: "w.lua".into(), report: serde_json::json!(null), elapsed_ms: 5, error: None }, "workflow_done"),
            (AgentEvent::ConvergeStarted { run_id, phase_id: 0, span_id: 1, items: 3, max_rounds: 3 }, "converge_started"),
            (AgentEvent::ConvergeDone { run_id, phase_id: 0, span_id: 1, rounds: 2, converged: true, surviving: 1, result: serde_json::json!({}), elapsed_ms: 5, error: None }, "converge_done"),
            (AgentEvent::PipelineStarted { run_id, total_stages: 1, items: 1 }, "pipeline_started"),
            (AgentEvent::PipelineStageStarted { run_id, stage_index: 0, label: "s".into(), agents_in_stage: 1 }, "pipeline_stage_started"),
            (AgentEvent::PipelineItemDone { run_id, stage_index: 0, item_index: 0, status: AgentStatus::Ok, tokens: TokenUsage::default(), elapsed_ms: 0 }, "pipeline_item_done"),
            (AgentEvent::PipelineDone { run_id, stages_completed: 1, total_ok: 1, total_failed: 0 }, "pipeline_done"),
        ];
        for (evt, expected) in cases {
            assert_eq!(event_type_name(&evt), expected);
        }
    }

    #[test]
    fn event_type_name_pipeline_events() {
        let run_id = RunId::now_v7();
        assert_eq!(event_type_name(&AgentEvent::PipelineStarted { run_id, total_stages: 3, items: 5 }), "pipeline_started");
        assert_eq!(event_type_name(&AgentEvent::PipelineStageStarted { run_id, stage_index: 1, label: "process".into(), agents_in_stage: 2 }), "pipeline_stage_started");
        assert_eq!(event_type_name(&AgentEvent::PipelineItemDone { run_id, stage_index: 2, item_index: 3, status: AgentStatus::Error, tokens: TokenUsage::default(), elapsed_ms: 100 }), "pipeline_item_done");
        assert_eq!(event_type_name(&AgentEvent::PipelineDone { run_id, stages_completed: 4, total_ok: 8, total_failed: 2 }), "pipeline_done");
    }

    #[test]
    fn default_capabilities_contains_expected() {
        let caps = default_capabilities();
        assert!(caps.contains(&"run"));
        assert!(caps.contains(&"subscribe"));
        assert!(caps.contains(&"cancel"));
        assert!(caps.contains(&"acp_raw"));
        assert!(caps.contains(&"sdk_events"));
        assert_eq!(caps.len(), 15);
    }

    #[test]
    fn run_payload_default_args() {
        let p: RunPayload = serde_json::from_str(
            r#"{"nl":"hi"}"#,
        ).unwrap();
        assert!(p.args.is_null());
        assert!(!p.confirm);
    }
}
