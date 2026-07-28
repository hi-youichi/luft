//! Service-layer response types.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct ExecuteWorkflowResponse {
    pub run_id: String,
    pub status: String,
    pub resumed_from: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowFile {
    pub name: String,
    pub path: String,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub task: String,
    pub status: String,
    pub total_tokens: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct ListRunsResponse {
    pub runs: Vec<RunSummary>,
    pub count: usize,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Serialize)]
pub struct PhaseAgentView {
    pub short_id: String,
    pub status: String,
    pub tokens: Option<u64>,
    pub findings: usize,
    pub last_message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PhaseView {
    pub phase_id: u32,
    pub label: String,
    pub status: String,
    pub planned: Option<usize>,
    pub ok: usize,
    pub failed: usize,
    pub agents: Vec<PhaseAgentView>,
}

#[derive(Debug, Serialize)]
pub struct RunStatusResponse {
    pub run_id: String,
    pub run_dir: String,
    pub task: String,
    pub status: String,
    pub current_phase: u32,
    pub completed_phases: usize,
    pub total_started: usize,
    pub completed_agents: usize,
    pub running_agents: usize,
    pub total_tokens: u64,
    pub created_at: String,
    pub updated_at: String,
    pub total_phases: usize,
    pub phases: Vec<PhaseView>,
    pub report: Value,
    pub error: Value,
}

#[derive(Debug, Serialize)]
pub struct RunEventsResponse {
    pub events: Vec<Value>,
    pub offset: u64,
    pub events_limit: u64,
    pub total_matching: u64,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CancelRunResponse {
    pub run_id: String,
    pub result: String,
    pub note: Option<String>,
}
