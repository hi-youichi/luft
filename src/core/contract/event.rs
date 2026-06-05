//! Event bus contract (§1.4) — the single observability data source.
//! headless serialises each [`AgentEvent`] as one JSONL line; the TUI projects
//! the same stream into a phase→agent view; the state store persists it.

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
    },
    RunDone {
        run_id: RunId,
        status: RunStatus,
        total_tokens: TokenUsage,
        report: serde_json::Value,
    },
    Log {
        run_id: RunId,
        agent_id: Option<AgentId>,
        level: LogLevel,
        msg: String,
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
