//! AgentBackend control-plane contract (§1.2). `core` ↔ `adapters` boundary.

use crate::core::contract::event::EventSender;
use crate::core::contract::finding::Finding;
use crate::core::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A pluggable agent backend (e.g. OpenCode via ACP). Prompt in, structured
/// result out. Implementations should observe `ctx.cancel` and return promptly
/// with [`BackendError::Cancelled`] when it fires.
#[async_trait]
pub trait AgentBackend: Send + Sync {
    /// Stable backend id, e.g. "opencode".
    fn id(&self) -> &'static str;

    /// Capability declaration (v0.1: recorded/validated only; routing in v0.2).
    fn capabilities(&self) -> AgentCapabilities;

    /// Run one agent task to completion.
    async fn run(&self, task: AgentTask, ctx: RunContext) -> Result<AgentResult, BackendError>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentCapabilities {
    pub streaming: bool,
    pub mcp_injection: bool,
    pub structured_output: bool,
    /// Known model ids; empty = unknown/any.
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTask {
    pub agent_id: AgentId,
    pub phase_id: PhaseId,
    pub prompt: String,
    pub model: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub agent_seq: u32,
    pub allowlist: Option<ToolPolicy>,
    pub workdir: PathBuf,
    /// Data-plane injection point (Maestro MCP endpoint).
    pub mcp_endpoint: Option<McpEndpoint>,
    /// Idle timeout: maximum silence (no ACP notifications) before the backend
    /// kills the session. `None` = backend default (5 min).
    pub timeout: Option<Duration>,
    /// Optional JSON Schema (M4) for validating agent output.
    /// When set, the runtime validates the agent's output against this schema
    /// and may retry or reject if validation fails.
    pub output_schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub agent_id: AgentId,
    pub status: AgentStatus,
    /// Structured output: prefers aggregated MCP findings, falls back to parsed
    /// final message.
    pub output: serde_json::Value,
    #[serde(default)]
    pub findings: Vec<Finding>,
    pub tokens_used: TokenUsage,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    pub logs: LogRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentStatus {
    Ok,
    Error,
    Cancelled,
    TimedOut,
}

/// Per-agent runtime context: cancellation + event sink + run association.
#[derive(Clone)]
pub struct RunContext {
    pub run_id: RunId,
    pub cancel: CancellationToken,
    pub events: EventSender,
}

/// Tool permission policy. v0.1 translates to a backend's acceptEdits + command
/// allowlist.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolPolicy {
    pub accept_edits: bool,
    pub allow_commands: Vec<String>,
    pub allow_mcp: Vec<String>,
    /// Explicit denies (highest precedence).
    pub deny: Vec<String>,
}

/// MCP data-plane endpoint injected into an agent for structured reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpEndpoint {
    /// Server name injected into the agent, e.g. "maestro".
    pub name: String,
    pub url: String,
    pub run_id: RunId,
    pub agent_id: AgentId,
    pub auth_token: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum BackendError {
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("connection error: {0}")]
    Connection(String),
    #[error("backend timed out")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("configuration error: {0}")]
    Config(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("execution error: {0}")]
    Execution(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl BackendError {
    /// Distinguish retryable (transient/timeout) from non-retryable (protocol/logic).
    pub fn is_retryable(&self) -> bool {
        matches!(self, BackendError::Timeout | BackendError::Spawn(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub key: String,
    pub path: Option<PathBuf>,
    pub inline: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogRef {
    pub path: PathBuf,
}
