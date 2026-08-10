//! Progress persistence and resume for long-running workflows.
//!
//! This module defines the data types for checkpointing and the
//! `CheckpointBackend` trait that persistence engines (e.g. SQLite) implement.
//!
//! Key features:
//! - Event log persistence
//! - Agent result caching
//! - Resume from last checkpoint
//! - Run state management

use crate::contract::event::AgentEvent;
use crate::contract::finding::Finding;
use crate::contract::ids::{AgentId, PhaseId, RunId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;


// ============================================================================
// Data Types (frozen contracts)
// ============================================================================

/// Run state persisted to the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCheckpoint {
    pub run_id: RunId,
    pub task: String,
    pub status: CheckpointStatus,
    pub current_phase: u32,
    pub completed_phases: Vec<PhaseSummary>,
    pub agent_results: HashMap<AgentId, AgentResultCache>,
    #[serde(default)]
    pub agent_sessions: HashMap<AgentId, AgentSessionCheckpoint>,
    pub findings: Vec<Finding>,
    pub total_tokens: u64,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub workflow_meta: Option<serde_json::Value>,
    #[serde(default)]
    pub started_agent_ids: Vec<AgentId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckpointStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for CheckpointStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CheckpointStatus::Running => "Running",
            CheckpointStatus::Completed => "Completed",
            CheckpointStatus::Failed => "Failed",
            CheckpointStatus::Cancelled => "Cancelled",
        };
        f.write_str(s)
    }
}

impl CheckpointStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CheckpointStatus::Running => "running",
            CheckpointStatus::Completed => "completed",
            CheckpointStatus::Failed => "failed",
            CheckpointStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "completed" => CheckpointStatus::Completed,
            "failed" => CheckpointStatus::Failed,
            "cancelled" => CheckpointStatus::Cancelled,
            _ => CheckpointStatus::Running,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseSummary {
    pub phase_id: PhaseId,
    pub label: String,
    pub planned: usize,
    pub ok: usize,
    pub failed: usize,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResultCache {
    pub agent_id: AgentId,
    pub phase_id: PhaseId,
    pub status: String,
    pub output: serde_json::Value,
    pub findings: Vec<Finding>,
    pub tokens: u64,
    pub completed_at: u64,
    #[serde(default)]
    pub cache_key_hash: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSessionCheckpoint {
    pub agent_id: AgentId,
    #[serde(default)]
    pub backend_id: Option<String>,
    #[serde(default)]
    pub protocol_session_id: Option<String>,
    pub session_id: String,
    pub status: String,
    pub updated_at: u64,
    #[serde(default)]
    pub resumable: bool,
}

// ============================================================================
// CheckpointBackend Trait
// ============================================================================

/// Persistence backend for a single run.
///
/// Implementations (e.g. `SqliteCheckpointBackend` in `luft-storage`)
/// provide checkpoint + event log storage. All methods are synchronous;
/// async backends bridge internally via `block_in_place`.
pub trait CheckpointBackend: Send + Sync + std::fmt::Debug {
    /// Initialize a new run.
    fn init_run(&self, run_id: RunId, task: &str, run_dir: &str) -> anyhow::Result<()>;

    /// Initialize a new run with declarative workflow metadata.
    fn init_run_with_meta(
        &self,
        run_id: RunId,
        task: &str,
        run_dir: &str,
        workflow_meta: serde_json::Value,
    ) -> anyhow::Result<()>;

    /// Open an existing run for resume. Returns None if not found.
    fn open_run(&self, run_id: RunId) -> anyhow::Result<Option<RunCheckpoint>>;

    /// Append an event to the log and update checkpoint state.
    fn append_event(&self, event: &AgentEvent) -> anyhow::Result<()>;

    /// Insert or update an agent result.
    fn upsert_agent_result(&self, cache: &AgentResultCache) -> anyhow::Result<()>;

    /// Insert or update an agent session.
    fn upsert_agent_session(&self, session: &AgentSessionCheckpoint) -> anyhow::Result<()>;

    /// Get current checkpoint (from in-memory cache).
    fn get_checkpoint(&self) -> Option<RunCheckpoint>;

    /// Get all findings.
    fn get_findings(&self) -> Vec<Finding>;

    /// Get event log as a vector.
    fn get_event_log(&self) -> anyhow::Result<Vec<AgentEvent>>;

    /// Check if a run can be resumed.
    fn can_resume(&self) -> bool;

    /// Reset checkpoint status to Running.
    fn reset_status_to_running(&self) -> anyhow::Result<()>;

    /// Mark run as cancelled.
    fn cancel(&self) -> anyhow::Result<()>;

    /// Save checkpoint (full overwrite).
    fn save_checkpoint(&self, checkpoint: &RunCheckpoint) -> anyhow::Result<()>;
}

/// Helper: current unix timestamp.
pub fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ============================================================================
// Factory — callers provide a backend at construction time.
// ============================================================================

/// List all run directory names under the base dir.
/// For SQLite backends, this is derived from the `runs` table.
/// This helper remains for filesystem-based discovery.
pub fn list_run_dirs(base_dir: &Path) -> anyhow::Result<Vec<String>> {
    if !base_dir.exists() {
        return Ok(vec![]);
    }
    let mut run_dirs = Vec::new();
    for entry in std::fs::read_dir(base_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                run_dirs.push(name.to_string());
            }
        }
    }
    run_dirs.sort();
    Ok(run_dirs)
}
