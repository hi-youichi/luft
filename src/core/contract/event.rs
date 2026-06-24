//! Event bus contract (§1.4) — the single observability data source.
//! Each [`AgentEvent`] is consumed by the event bus subscribers; the state store persists it.

use crate::core::contract::backend::AgentStatus;
use crate::core::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Broadcast sender shared by the scheduler and every event producer.
pub type EventSender = tokio::sync::broadcast::Sender<AgentEvent>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    RunStarted {
        run_id: RunId,
        task: String,
        ts: DateTime<Utc>,
    },
    PhaseStarted {
        run_id: RunId,
        phase_id: PhaseId,
        label: String,
        planned: usize,
        #[serde(default)]
        ts: DateTime<Utc>,
    },
    AgentStarted {
        run_id: RunId,
        phase_id: PhaseId,
        agent_id: AgentId,
        prompt_preview: String,
        model: Option<String>,
    },
    AgentProgress {
        run_id: RunId,
        agent_id: AgentId,
        delta: ProgressDelta,
    },
    /// Raw ACP `session/update` passthrough — the verbatim notification, surfaced
    /// for observability. Produced only when the ACP backend has raw events
    /// enabled. Not persisted to
    /// the journal (see `acp-raw-events.md`).
    AcpRaw {
        run_id: RunId,
        agent_id: AgentId,
        /// `SessionUpdate` discriminator (the `sessionUpdate` tag), e.g.
        /// `"agent_message_chunk"`, `"plan"` — lets consumers filter without
        /// parsing `raw`.
        kind: String,
        /// The ACP `SessionUpdate`, serialized verbatim.
        raw: serde_json::Value,
    },
    AgentDone {
        run_id: RunId,
        agent_id: AgentId,
        status: AgentStatus,
        tokens: TokenUsage,
        elapsed_ms: u64,
    },
    PhaseDone {
        run_id: RunId,
        phase_id: PhaseId,
        ok: usize,
        failed: usize,
        #[serde(default)]
        ts: DateTime<Utc>,
    },
    RunDone {
        run_id: RunId,
        status: RunStatus,
        total_tokens: TokenUsage,
        report: serde_json::Value,
        #[serde(default)]
        ts: DateTime<Utc>,
    },
    Log {
        run_id: RunId,
        agent_id: Option<AgentId>,
        level: LogLevel,
        msg: String,
    },
    // SDK primitive events (§ sdk-events.md) — DSL-granularity observability for
    // the orchestration script. Blocking primitives emit a Started/Done span
    // pair correlated by `span_id`; instantaneous ones emit a single event.
    BudgetSet {
        run_id: RunId,
        time_limit_ms: Option<u64>,
        max_rounds: Option<u32>,
    },
    ReportEmitted {
        run_id: RunId,
        phase_id: PhaseId,
        report: serde_json::Value,
    },
    ParallelStarted {
        run_id: RunId,
        phase_id: PhaseId,
        span_id: u64,
        count: usize,
    },
    ParallelDone {
        run_id: RunId,
        phase_id: PhaseId,
        span_id: u64,
        ok: usize,
        failed: usize,
        results: serde_json::Value,
        elapsed_ms: u64,
    },
    WorkflowStarted {
        run_id: RunId,
        span_id: u64,
        path: String,
        args: serde_json::Value,
    },
    WorkflowDone {
        run_id: RunId,
        span_id: u64,
        path: String,
        report: serde_json::Value,
        elapsed_ms: u64,
        error: Option<String>,
    },
    ConvergeStarted {
        run_id: RunId,
        phase_id: PhaseId,
        span_id: u64,
        items: usize,
        max_rounds: u32,
    },
    ConvergeDone {
        run_id: RunId,
        phase_id: PhaseId,
        span_id: u64,
        rounds: u32,
        converged: bool,
        surviving: usize,
        result: serde_json::Value,
        elapsed_ms: u64,
        error: Option<String>,
    },
    // M2 Pipeline events
    PipelineStarted {
        run_id: RunId,
        total_stages: usize,
        items: usize,
    },
    PipelineStageStarted {
        run_id: RunId,
        stage_index: usize,
        label: String,
        agents_in_stage: usize,
    },
    PipelineItemDone {
        run_id: RunId,
        stage_index: usize,
        item_index: usize,
        status: AgentStatus,
        tokens: TokenUsage,
        elapsed_ms: u64,
    },
    PipelineDone {
        run_id: RunId,
        stages_completed: usize,
        total_ok: usize,
        total_failed: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProgressDelta {
    Message { text: String },
    ToolCall { name: String, summary: String },
    FileEdit { path: PathBuf },
    Tokens { usage: TokenUsage },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Failed,
    Cancelled,
    Partial,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}
