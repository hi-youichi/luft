//! Progress persistence and resume for long-running workflows.
//!
//! This module implements checkpointing and recovery for dynamic workflows.
//! Progress is saved as the run goes, so a job interrupted by a restart can resume.
//!
//! Key features:
//! - Event log persistence (JSONL)
//! - Agent result caching
//! - Resume from last checkpoint
//! - Run state management

use crate::core::contract::event::AgentEvent;
use crate::core::contract::finding::Finding;
use crate::core::contract::ids::{AgentId, PhaseId, RunId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Run state persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCheckpoint {
    pub run_id: RunId,
    pub task: String,
    pub status: CheckpointStatus,
    pub current_phase: u32,
    pub completed_phases: Vec<PhaseSummary>,
    pub agent_results: HashMap<AgentId, AgentResultCache>,
    pub findings: Vec<Finding>,
    pub total_tokens: u64,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub completed_spans: Vec<PhaseSpanSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckpointStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
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
    /// Deterministic cache key hash for resume lookups.
    /// Populated by JournalStore::cache_agent(); None for legacy checkpoints.
    #[serde(default)]
    pub cache_key_hash: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseSpanSummary {
    pub id: u32,
    pub name: String,
    pub parent_id: Option<u32>,
    pub depth: u32,
    pub elapsed_ms: u64,
    pub completed_at: u64,
}

/// Persistence store for a single run.
#[derive(Debug)]
pub struct RunStore {
    run_dir: PathBuf,
    checkpoint: RwLock<Option<RunCheckpoint>>,
    events_file: RwLock<Option<File>>,
}

impl RunStore {
    /// Create or open a run store at the given path.
    pub fn new(run_dir: &Path) -> Result<Arc<Self>, std::io::Error> {
        tracing::debug!(path = %run_dir.display(), "creating RunStore");
        fs::create_dir_all(run_dir)?;

        let store = Arc::new(Self {
            run_dir: run_dir.to_path_buf(),
            checkpoint: RwLock::new(None),
            events_file: RwLock::new(None),
        });

        Ok(store)
    }

    /// Insert or update an agent result in the checkpoint directly.
    /// Used by JournalStore to persist cache_key_hash before appending the event.
    pub fn upsert_agent_result(&self, cache: &AgentResultCache) -> Result<(), std::io::Error> {
        let mut guard = self.checkpoint.write().unwrap();
        if let Some(ref mut checkpoint) = *guard {
            checkpoint.agent_results.insert(cache.agent_id, cache.clone());
            checkpoint.updated_at = current_timestamp();
            let cp = checkpoint.clone();
            drop(guard);
            let cp_path = self.run_dir.join("checkpoint.json");
            let content = serde_json::to_string_pretty(&cp)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            fs::write(&cp_path, content)?;
        }
        Ok(())
    }

    /// Initialize a new run.
    pub fn init_run(&self, run_id: RunId, task: &str) -> Result<(), std::io::Error> {
        tracing::info!(%run_id, %task, "initializing run store");
        let checkpoint = RunCheckpoint {
            run_id,
            task: task.to_string(),
            status: CheckpointStatus::Running,
            current_phase: 0,
            completed_phases: vec![],
            agent_results: HashMap::new(),
            findings: vec![],
            total_tokens: 0,
            created_at: current_timestamp(),
            updated_at: current_timestamp(),
            completed_spans: vec![],
        };

        // Save checkpoint
        self.save_checkpoint(&checkpoint)?;

        // Open events file
        let events_path = self.run_dir.join("events.jsonl");
        let events_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(events_path)?;

        let mut checkpoint_guard = self.checkpoint.write().unwrap();
        *checkpoint_guard = Some(checkpoint);

        let mut events_guard = self.events_file.write().unwrap();
        *events_guard = Some(events_file);

        Ok(())
    }

    /// Open an existing run for resume.
    pub fn open_run(&self, _run_id: RunId) -> Result<Option<RunCheckpoint>, std::io::Error> {
        tracing::debug!(%_run_id, "opening existing run");
        let checkpoint_path = self.run_dir.join("checkpoint.json");

        if !checkpoint_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&checkpoint_path)?;
        let checkpoint: RunCheckpoint = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        // Open events file
        let events_path = self.run_dir.join("events.jsonl");
        let events_file = OpenOptions::new()
            .read(true)
            .open(events_path)?;

        let mut checkpoint_guard = self.checkpoint.write().unwrap();
        *checkpoint_guard = Some(checkpoint.clone());

        let mut events_guard = self.events_file.write().unwrap();
        *events_guard = Some(events_file);

        Ok(Some(checkpoint))
    }

    /// Append an event to the log.
    pub fn append_event(&self, event: &AgentEvent) -> Result<(), std::io::Error> {
        let json = serde_json::to_string(event)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let mut events_guard = self.events_file.write().unwrap();
        if let Some(ref mut file) = *events_guard {
            writeln!(file, "{}", json)?;
            file.flush()?;
        }

        // Update checkpoint (this also persists to disk)
        self.update_from_event(event);

        Ok(())
    }

    /// Update checkpoint from an event and persist to disk.
    fn update_from_event(&self, event: &AgentEvent) {
        let mut checkpoint_guard = self.checkpoint.write().unwrap();
        if let Some(ref mut checkpoint) = *checkpoint_guard {
            match event {
                AgentEvent::AgentDone { agent_id, status, tokens, .. } => {
                    let existing = checkpoint.agent_results.get(agent_id);
                    let cache = AgentResultCache {
                        agent_id: *agent_id,
                        phase_id: existing.map(|c| c.phase_id).unwrap_or(0),
                        status: format!("{:?}", status).to_lowercase(),
                        output: existing
                            .map(|c| c.output.clone())
                            .unwrap_or(serde_json::Value::Null),
                        findings: existing
                            .map(|c| c.findings.clone())
                            .unwrap_or_default(),
                        tokens: tokens.total(),
                        completed_at: existing
                            .map(|c| c.completed_at)
                            .unwrap_or(current_timestamp()),
                        cache_key_hash: existing.and_then(|c| c.cache_key_hash.clone()),
                        description: existing.and_then(|c| c.description.clone()),
                        role: existing.and_then(|c| c.role.clone()),
                    };
                    checkpoint.agent_results.insert(*agent_id, cache);
                    checkpoint.total_tokens += tokens.total();
                }
                AgentEvent::PhaseDone { phase_id, .. } => {
                    if *phase_id > 0 {
                        checkpoint.current_phase = *phase_id;
                    }
                }
                AgentEvent::PhaseSpanDone { span_id, name, parent_id, depth, elapsed_ms, .. } => {
                    checkpoint.completed_spans.push(PhaseSpanSummary {
                        id: *span_id,
                        name: name.clone(),
                        parent_id: *parent_id,
                        depth: *depth,
                        elapsed_ms: *elapsed_ms,
                        completed_at: current_timestamp(),
                    });
                }
                AgentEvent::RunDone { status, total_tokens, .. } => {
                    checkpoint.status = match status {
                        crate::core::contract::event::RunStatus::Completed => CheckpointStatus::Completed,
                        crate::core::contract::event::RunStatus::Failed => CheckpointStatus::Failed,
                        crate::core::contract::event::RunStatus::Cancelled => CheckpointStatus::Cancelled,
                        crate::core::contract::event::RunStatus::Partial => CheckpointStatus::Running,
                    };
                    // Only overwrite if a real total was supplied; otherwise keep
                    // the figure accumulated from AgentDone events.
                    let t = total_tokens.total();
                    if t > 0 {
                        checkpoint.total_tokens = t;
                    }
                }
                _ => {}
            }
            checkpoint.updated_at = current_timestamp();

            // Persist updated checkpoint to disk (write-only, no lock needed - already held)
            if let Err(e) = self.write_checkpoint_to_disk(checkpoint) {
                tracing::warn!(error = %e, "failed to save checkpoint");
            }
        }
    }

    /// Write checkpoint to disk without acquiring any locks.
    fn write_checkpoint_to_disk(&self, checkpoint: &RunCheckpoint) -> Result<(), std::io::Error> {
        let checkpoint_path = self.run_dir.join("checkpoint.json");
        let content = serde_json::to_string_pretty(checkpoint)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(&checkpoint_path, content)
    }

    /// Save checkpoint to disk (public API, acquires lock).
    pub fn save_checkpoint(&self, checkpoint: &RunCheckpoint) -> Result<(), std::io::Error> {
        let checkpoint_path = self.run_dir.join("checkpoint.json");
        let content = serde_json::to_string_pretty(checkpoint)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(&checkpoint_path, content)?;

        let mut checkpoint_guard = self.checkpoint.write().unwrap();
        *checkpoint_guard = Some(checkpoint.clone());

        Ok(())
    }

    /// Get current checkpoint.
    pub fn get_checkpoint(&self) -> Option<RunCheckpoint> {
        let guard = self.checkpoint.read().unwrap();
        guard.clone()
    }

    /// Get cached agent results.
    pub fn get_agent_results(&self) -> HashMap<AgentId, AgentResultCache> {
        let guard = self.checkpoint.read().unwrap();
        guard
            .as_ref()
            .map(|c| c.agent_results.clone())
            .unwrap_or_default()
    }

    /// Get all findings collected so far.
    pub fn get_findings(&self) -> Vec<Finding> {
        let guard = self.checkpoint.read().unwrap();
        guard
            .as_ref()
            .map(|c| c.findings.clone())
            .unwrap_or_default()
    }

    /// Get event log as a vector.
    pub fn get_event_log(&self) -> Result<Vec<AgentEvent>, std::io::Error> {
        let events_path = self.run_dir.join("events.jsonl");
        let file = File::open(events_path)?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let event: AgentEvent = serde_json::from_str(&line)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            events.push(event);
        }

        Ok(events)
    }

    /// Check if a run can be resumed.
    pub fn can_resume(&self) -> bool {
        let guard = self.checkpoint.read().unwrap();
        matches!(
            guard.as_ref().map(|c| c.status.clone()),
            Some(CheckpointStatus::Running)
        )
    }

    /// Mark run as cancelled.
    pub fn cancel(&self) -> Result<(), std::io::Error> {
        tracing::info!("cancelling run");
        let mut guard = self.checkpoint.write().unwrap();
        if let Some(ref mut checkpoint) = *guard {
            checkpoint.status = CheckpointStatus::Cancelled;
            checkpoint.updated_at = current_timestamp();
            drop(guard);
            let guard = self.checkpoint.read().unwrap();
            if let Some(ref c) = *guard {
                let checkpoint_path = self.run_dir.join("checkpoint.json");
                let content = serde_json::to_string_pretty(c)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                fs::write(&checkpoint_path, content)?;
            }
        }
        Ok(())
    }
}

/// Get current timestamp.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ============================================================================
// Global store management
// ============================================================================

use std::sync::OnceLock;

static RUN_STORES: OnceLock<dashmap::DashMap<String, Arc<RunStore>>> = OnceLock::new();

/// Get or create the global run stores.
fn get_run_stores() -> &'static dashmap::DashMap<String, Arc<RunStore>> {
    RUN_STORES.get_or_init(|| dashmap::DashMap::new())
}

/// Get or create a run store for a run directory.
pub fn get_run_store(run_dir_name: &str, base_dir: &Path) -> Result<Arc<RunStore>, std::io::Error> {
    let stores = get_run_stores();

    if let Some(store) = stores.get(run_dir_name) {
        return Ok(store.clone());
    }

    let run_dir = base_dir.join(run_dir_name);
    let store = RunStore::new(&run_dir)?;
    stores.insert(run_dir_name.to_string(), store.clone());

    Ok(store)
}

/// List all run directory names (both new-format and legacy UUID).
pub fn list_runs(base_dir: &Path) -> Result<Vec<String>, std::io::Error> {
    if !base_dir.exists() {
        return Ok(vec![]);
    }

    let mut run_dirs = Vec::new();
    for entry in fs::read_dir(base_dir)? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_run_store_init() {
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "Test task").unwrap();

        let checkpoint = store.get_checkpoint().unwrap();
        assert_eq!(checkpoint.run_id, run_id);
        assert_eq!(checkpoint.task, "Test task");
        assert_eq!(checkpoint.status, CheckpointStatus::Running);
    }

    #[test]
    fn test_run_store_resume() {
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "Test task").unwrap();

        // Open in new store instance
        let store2 = RunStore::new(dir.path()).unwrap();
        let checkpoint = store2.open_run(run_id).unwrap().unwrap();
        assert_eq!(checkpoint.run_id, run_id);
        assert_eq!(checkpoint.task, "Test task");
    }

    #[test]
    fn test_can_resume() {
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "Test task").unwrap();

        assert!(store.can_resume());
    }
}