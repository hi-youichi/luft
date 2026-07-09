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
            checkpoint
                .agent_results
                .insert(cache.agent_id, cache.clone());
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

        // Open events file. Resume appends new events (phase_started, agent_started,
        // log, agent_done) to the same file; opening read-only here would make every
        // forwarded event fail with Access is denied (os error 5) and silently drop
        // observability for the entire resumed run.
        let events_path = self.run_dir.join("events.jsonl");
        let events_file = OpenOptions::new()
            .read(true)
            .append(true)
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
                AgentEvent::AgentDone {
                    agent_id,
                    status,
                    tokens,
                    ..
                } => {
                    let existing = checkpoint.agent_results.get(agent_id);
                    let cache = AgentResultCache {
                        agent_id: *agent_id,
                        phase_id: existing.map(|c| c.phase_id).unwrap_or(0),
                        status: format!("{:?}", status).to_lowercase(),
                        output: existing
                            .map(|c| c.output.clone())
                            .unwrap_or(serde_json::Value::Null),
                        findings: existing.map(|c| c.findings.clone()).unwrap_or_default(),
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
                AgentEvent::PhaseSpanDone {
                    span_id,
                    name,
                    parent_id,
                    depth,
                    elapsed_ms,
                    ..
                } => {
                    checkpoint.completed_spans.push(PhaseSpanSummary {
                        id: *span_id,
                        name: name.clone(),
                        parent_id: *parent_id,
                        depth: *depth,
                        elapsed_ms: *elapsed_ms,
                        completed_at: current_timestamp(),
                    });
                }
                AgentEvent::RunDone {
                    status,
                    total_tokens,
                    ..
                } => {
                    checkpoint.status = match status {
                        crate::core::contract::event::RunStatus::Completed => {
                            CheckpointStatus::Completed
                        }
                        crate::core::contract::event::RunStatus::Failed => CheckpointStatus::Failed,
                        crate::core::contract::event::RunStatus::Cancelled => {
                            CheckpointStatus::Cancelled
                        }
                        crate::core::contract::event::RunStatus::Partial => {
                            CheckpointStatus::Running
                        }
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
    RUN_STORES.get_or_init(dashmap::DashMap::new)
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

    #[test]
    fn test_resume_appends_events() {
        // Regression: open_run previously opened events.jsonl read-only, causing
        // every forwarded event in the resumed run to fail with
        // `Access is denied (os error 5)` and silently dropping observability.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "Test task").unwrap();

        let store2 = RunStore::new(dir.path()).unwrap();
        store2.open_run(run_id).unwrap().unwrap();

        // Writing through the resumed store must succeed and persist the event.
        let evt = AgentEvent::Log {
            run_id,
            agent_id: None,
            level: crate::core::contract::event::LogLevel::Info,
            msg: "resume smoke test".to_string(),
        };
        store2
            .append_event(&evt)
            .expect("append_event after resume must succeed");

        let log = store2.get_event_log().expect("read events.jsonl");
        assert!(
            log.iter().any(|e| matches!(
                e,
                AgentEvent::Log { msg, .. } if msg == "resume smoke test"
            )),
"event written after open_run must appear in events.jsonl"
        );
    }

    // ----------------------------------------------------------------------
    // Tests for F1 / F4 / F5 / F8 (spec `docs/src/core/state.rs.md`).
    //
    // These exercise the consolidated write path
    // (`write_checkpoint_to_disk`), the lock-dance-free `cancel`, the
    // snake_case `AgentStatus::as_str()` mapping that no longer depends on
    // `Debug` formatting, and the `serde_to_io` error mapping helper that
    // funnels every `serde_json::Error` through `ErrorKind::InvalidData`.
    // ----------------------------------------------------------------------

    use crate::core::contract::backend::AgentStatus;
    use crate::core::contract::ids::TokenUsage;
    use std::collections::HashSet;

    fn sample_token_usage() -> TokenUsage {
        TokenUsage {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
        }
    }

    fn build_agent_done(
        run_id: RunId,
        agent_id: AgentId,
        status: AgentStatus,
        tokens: TokenUsage,
    ) -> AgentEvent {
        AgentEvent::AgentDone {
            run_id,
            agent_id,
            status,
            tokens,
            elapsed_ms: 0,
            name: None,
            agent_seq: 0,
            output: serde_json::Value::Null,
            findings: vec![],
            prompt: String::new(),
            retry_count: 0,
        }
    }

    fn read_raw_checkpoint(run_dir: &Path) -> serde_json::Value {
        let path = run_dir.join("checkpoint.json");
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read checkpoint.json: {e}"));
        serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("parse checkpoint.json: {e}"))
    }

    // ----- upsert_agent_result (F1 delegation) ---------------------------

    #[test]
    fn upsert_agent_result_persists_to_disk() {
        // F1: `upsert_agent_result` must persist via the same write path as
        // `write_checkpoint_to_disk` so that a follow-up `open_run` sees the
        // inserted entry without any in-process plumbing.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "upsert test").unwrap();

        let agent_id = uuid::Uuid::now_v7();
        let cache = AgentResultCache {
            agent_id,
            phase_id: 1,
            status: "ok".into(),
            output: serde_json::json!({"v": 42}),
            findings: vec![],
            tokens: 100,
            completed_at: 1_700_000_000,
            cache_key_hash: Some("deadbeef".into()),
            description: None,
            role: None,
        };
        store.upsert_agent_result(&cache).unwrap();

        // 1. In-memory state reflects the upsert.
        let cp = store.get_checkpoint().expect("checkpoint present");
        let cached = cp
            .agent_results
            .get(&agent_id)
            .expect("agent_id indexed after upsert");
        assert_eq!(cached.tokens, 100);
        assert_eq!(cached.status, "ok");

        // 2. On-disk JSON matches the in-memory state.
        let raw = read_raw_checkpoint(dir.path());
        let ar = raw
            .get("agent_results")
            .and_then(|v| v.as_object())
            .expect("agent_results object");
        assert_eq!(ar.len(), 1, "exactly one agent cached on disk");
        let entry = ar
            .values()
            .next()
            .expect("non-empty agent_results on disk");
        assert_eq!(entry.get("tokens").and_then(|v| v.as_u64()), Some(100));
        assert_eq!(entry.get("status").and_then(|v| v.as_str()), Some("ok"));
        assert_eq!(
            entry.get("cache_key_hash").and_then(|v| v.as_str()),
            Some("deadbeef")
        );

        // 3. Re-opening the run restores the entry from disk.
        drop(store);
        let reopened = RunStore::new(dir.path()).unwrap();
        let restored = reopened.open_run(run_id).unwrap().unwrap();
        assert!(
            restored.agent_results.contains_key(&agent_id),
            "upserted entry must survive close+reopen"
        );
        assert_eq!(restored.agent_results[&agent_id].tokens, 100);
    }

    #[test]
    fn upsert_agent_result_updates_existing_entry() {
        // F1: re-upserting the same agent_id overwrites the prior entry,
        // mirroring the HashMap semantics of agent_results.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "overwrite test").unwrap();

        let agent_id = uuid::Uuid::now_v7();
        let first = AgentResultCache {
            agent_id,
            phase_id: 1,
            status: "ok".into(),
            output: serde_json::json!("first"),
            findings: vec![],
            tokens: 10,
            completed_at: 1,
            cache_key_hash: None,
            description: None,
            role: None,
        };
        let second = AgentResultCache {
            agent_id,
            phase_id: 1,
            status: "error".into(),
            output: serde_json::json!("second"),
            findings: vec![],
            tokens: 99,
            completed_at: 2,
            cache_key_hash: None,
            description: None,
            role: None,
        };
        store.upsert_agent_result(&first).unwrap();
        store.upsert_agent_result(&second).unwrap();

        let cp = store.get_checkpoint().unwrap();
        assert_eq!(cp.agent_results.len(), 1, "no duplicate entries");
        let cached = &cp.agent_results[&agent_id];
        assert_eq!(cached.status, "error");
        assert_eq!(cached.tokens, 99);
        assert_eq!(cached.completed_at, 2);

        // Disk must also reflect the second upsert, not the first.
        let raw = read_raw_checkpoint(dir.path());
        let ar = raw.get("agent_results").and_then(|v| v.as_object()).unwrap();
        assert_eq!(ar.len(), 1);
        let entry = ar.values().next().unwrap();
        assert_eq!(entry.get("tokens").and_then(|v| v.as_u64()), Some(99));
        assert_eq!(entry.get("status").and_then(|v| v.as_str()), Some("error"));
    }

    #[test]
    fn upsert_agent_result_noop_when_uninitialized() {
        // F1: before init_run the in-memory checkpoint is None and the helper
        // must not create a checkpoint.json from nothing. This keeps the
        // behaviour of "upsert only patches an existing checkpoint".
        let dir = tempdir().unwrap();
        let store = RunStore::new(dir.path()).unwrap();
        assert!(store.get_checkpoint().is_none());
        let cp_path = dir.path().join("checkpoint.json");
        assert!(!cp_path.exists(), "no checkpoint.json before init");

        let cache = AgentResultCache {
            agent_id: uuid::Uuid::now_v7(),
            phase_id: 1,
            status: "ok".into(),
            output: serde_json::json!(null),
            findings: vec![],
            tokens: 0,
            completed_at: 0,
            cache_key_hash: None,
            description: None,
            role: None,
        };
        store.upsert_agent_result(&cache).unwrap();
        assert!(
            !cp_path.exists(),
            "upsert_agent_result must not create checkpoint.json before init_run"
        );
        assert!(store.get_checkpoint().is_none());
    }

    #[test]
    fn upsert_agent_result_advances_updated_at() {
        // F1: the delegated write path must still update the checkpoint's
        // `updated_at` timestamp the same way the inline implementation did.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "ts test").unwrap();
        let before = store.get_checkpoint().unwrap().updated_at;

        std::thread::sleep(std::time::Duration::from_millis(1100));

        let cache = AgentResultCache {
            agent_id: uuid::Uuid::now_v7(),
            phase_id: 1,
            status: "ok".into(),
            output: serde_json::json!(null),
            findings: vec![],
            tokens: 0,
            completed_at: 0,
            cache_key_hash: None,
            description: None,
            role: None,
        };
        store.upsert_agent_result(&cache).unwrap();
        let after = store.get_checkpoint().unwrap().updated_at;
        assert!(
            after > before,
            "updated_at must advance after upsert (before={before}, after={after})"
        );
    }

    // ----- cancel (F1 delegation + F4 lock-dance collapse) ---------------

    #[test]
    fn cancel_persists_cancelled_status_to_disk() {
        // F1+F4: cancel delegates to write_checkpoint_to_disk with the
        // already-mutated checkpoint (no redundant read-lock + inline
        // serialize). The Cancelled status must appear on disk so a follow-up
        // process sees the terminal state.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "cancel me").unwrap();
        assert!(store.can_resume());

        store.cancel().unwrap();

        // In-memory: status is Cancelled, can_resume() is false.
        let cp = store.get_checkpoint().unwrap();
        assert_eq!(cp.status, CheckpointStatus::Cancelled);
        assert!(!store.can_resume());

        // On-disk: same status, observable across processes.
        let raw = read_raw_checkpoint(dir.path());
        assert_eq!(raw.get("status").and_then(|v| v.as_str()), Some("cancelled"));

        // Reopen: the persisted status survives close+reopen.
        drop(store);
        let reopened = RunStore::new(dir.path()).unwrap();
        let restored = reopened.open_run(run_id).unwrap().unwrap();
        assert_eq!(restored.status, CheckpointStatus::Cancelled);
        assert!(!reopened.can_resume());
    }

    #[test]
    fn cancel_is_idempotent() {
        // F4: the new cancel body only mutates under the write lock and
        // delegates to write_checkpoint_to_disk once. Calling it twice must
        // not panic, not deadlock, and must leave the persisted state
        // consistent (Cancelled, monotonically newer updated_at).
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "double cancel").unwrap();

        store.cancel().unwrap();
        let after_first = store.get_checkpoint().unwrap().updated_at;
        std::thread::sleep(std::time::Duration::from_millis(1100));

        store.cancel().expect("second cancel must succeed");
        let after_second = store.get_checkpoint().unwrap().updated_at;

        assert_eq!(
            store.get_checkpoint().unwrap().status,
            CheckpointStatus::Cancelled
        );
        assert!(
            after_second >= after_first,
            "updated_at must not regress (was {after_first}, now {after_second})"
        );

        let raw = read_raw_checkpoint(dir.path());
        assert_eq!(raw.get("status").and_then(|v| v.as_str()), Some("cancelled"));
    }

    #[test]
    fn cancel_before_init_is_safe_noop() {
        // F4: cancel on an uninitialised store must not panic and must not
        // create a checkpoint file. The cancelled-status guard requires
        // `Some(checkpoint)` so the body simply skips.
        let dir = tempdir().unwrap();
        let store = RunStore::new(dir.path()).unwrap();
        assert!(store.get_checkpoint().is_none());
        store.cancel().expect("cancel before init must succeed");
        assert!(store.get_checkpoint().is_none());
        assert!(
            !dir.path().join("checkpoint.json").exists(),
            "cancel before init must not create checkpoint.json"
        );
    }

    #[test]
    fn cancel_preserves_agent_results_and_findings() {
        // F1+F4: cancel only mutates status/updated_at. Pre-existing
        // agent_results, findings, and total_tokens must be preserved
        // verbatim across the cancel write.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "preserve").unwrap();

        let agent_id = uuid::Uuid::now_v7();
        let cache = AgentResultCache {
            agent_id,
            phase_id: 1,
            status: "ok".into(),
            output: serde_json::json!({"x": 1}),
            findings: vec![],
            tokens: 250,
            completed_at: 7,
            cache_key_hash: Some("hash-1".into()),
            description: None,
            role: None,
        };
        store.upsert_agent_result(&cache).unwrap();
        let before = store.get_checkpoint().unwrap();

        store.cancel().unwrap();
        let after = store.get_checkpoint().unwrap();

        assert_eq!(after.status, CheckpointStatus::Cancelled);
        assert_eq!(after.agent_results.len(), 1);
        assert_eq!(after.agent_results[&agent_id].tokens, 250);
        assert_eq!(
            after.agent_results[&agent_id].cache_key_hash.as_deref(),
            Some("hash-1")
        );
        assert_eq!(after.total_tokens, before.total_tokens);
    }

    // ----- AgentDone -> AgentResultCache.status (F5) ---------------------

    #[test]
    fn agent_done_persists_snake_case_status_for_each_variant() {
        // F5 KEY test: the persisted AgentResultCache.status string MUST
        // come from AgentStatus::as_str() and NOT from Debug formatting.
        // For TimedOut this is the load-bearing regression: Debug lowercased
        // yields "timedout" (no underscore) but as_str() yields "timed_out".
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "F5 variants").unwrap();

        let cases: Vec<(AgentStatus, &str)> = vec![
            (AgentStatus::Ok, "ok"),
            (AgentStatus::Error, "error"),
            (AgentStatus::Cancelled, "cancelled"),
            (AgentStatus::TimedOut, "timed_out"),
        ];
        for (status, expected) in &cases {
            let agent_id = uuid::Uuid::now_v7();
            let evt = build_agent_done(run_id, agent_id, status.clone(), sample_token_usage());
            store.append_event(&evt).unwrap();

            let raw = read_raw_checkpoint(dir.path());
            let ar = raw
                .get("agent_results")
                .and_then(|v| v.as_object())
                .expect("agent_results object");
            let entry = ar
                .values()
                .find(|v| {
                    v.get("agent_id").and_then(|id| id.as_str())
                        == Some(&agent_id.to_string())
                })
                .unwrap_or_else(|| panic!("entry for {agent_id} missing"));
            let persisted = entry
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("status missing for {status:?}"));
            assert_eq!(
                persisted, *expected,
                "AgentDone({status:?}) must persist status={expected:?} (snake_case); \
                 got {persisted:?}. If this fails with \"timedout\" for TimedOut, \
                 F5 has regressed to Debug formatting."
            );
        }
    }

    #[test]
    fn agent_done_timed_out_persists_with_underscore_not_collapsed() {
        // Strongest F5 regression guard: the buggy form would persist
        // "timedout" (no underscore) for TimedOut. The fixed form persists
        // "timed_out". This test fails loudly if anyone reintroduces the
        // `format!("{:?}", status).to_lowercase()` shortcut.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "timed-out guard").unwrap();

        let agent_id = uuid::Uuid::now_v7();
        let evt = build_agent_done(run_id, agent_id, AgentStatus::TimedOut, sample_token_usage());
        store.append_event(&evt).unwrap();

        let raw = read_raw_checkpoint(dir.path());
        let ar = raw.get("agent_results").and_then(|v| v.as_object()).unwrap();
        let entry = ar.values().next().expect("entry exists");
        let persisted = entry.get("status").and_then(|v| v.as_str()).unwrap();

        assert_eq!(
            persisted, "timed_out",
            "AgentDone(TimedOut) must persist \"timed_out\" with an underscore; got {persisted:?}"
        );
        assert_ne!(
            persisted, "timedout",
            "AgentDone(TimedOut) must NOT collapse to Debug-lowercased \"timedout\""
        );
    }

    #[test]
    fn agent_done_then_reopen_restores_snake_case_status() {
        // The persisted snake_case status must survive a close+reopen cycle,
        // since legacy checkpoints with Debug-lowercased "timedout" should be
        // distinguished from new checkpoints with "timed_out" — but new
        // checkpoints must round-trip cleanly through the JSON pipeline.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "round-trip").unwrap();

        let agent_id = uuid::Uuid::now_v7();
        let evt = build_agent_done(
            run_id,
            agent_id,
            AgentStatus::Cancelled,
            TokenUsage {
                input: 1,
                output: 2,
                cache_read: 0,
                cache_write: 0,
            },
        );
        store.append_event(&evt).unwrap();
        drop(store);

        let reopened = RunStore::new(dir.path()).unwrap();
        let cp = reopened.open_run(run_id).unwrap().unwrap();
        let cached = cp
            .agent_results
            .get(&agent_id)
            .expect("agent cached on disk");
        assert_eq!(cached.status, "cancelled");
        assert_eq!(cached.tokens, 3);
    }

    // ----- F8 serde_to_io error mapping (indirect) -----------------------

    #[test]
    fn open_run_with_corrupt_checkpoint_returns_invalid_data() {
        // F8: every serde_json::Error → io::Error funnel passes through
        // ErrorKind::InvalidData. Verifies the consolidated helper is wired
        // into open_run's deserialization path.
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(
            dir.path().join("checkpoint.json"),
            b"{ this is not valid json",
        )
        .unwrap();

        let store = RunStore::new(dir.path()).unwrap();
        let err = store
            .open_run(uuid::Uuid::now_v7())
            .expect_err("corrupt JSON must surface as an io::Error");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::InvalidData,
            "corrupt checkpoint must map to InvalidData via serde_to_io; got {:?}",
            err.kind()
        );
    }

    #[test]
    fn open_run_with_wrong_typed_checkpoint_returns_invalid_data() {
        // F8: even structurally-valid JSON that fails typed deserialisation
        // (missing required field) must come back as InvalidData.
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        // `task` is a required field on RunCheckpoint; omitting it triggers
        // a serde error which the helper must classify as InvalidData.
        std::fs::write(
            dir.path().join("checkpoint.json"),
            br#"{"run_id":"00000000-0000-0000-0000-000000000000","status":"running"}"#,
        )
        .unwrap();

        let store = RunStore::new(dir.path()).unwrap();
        let err = store
            .open_run(uuid::Uuid::now_v7())
            .expect_err("missing-field JSON must surface as an io::Error");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn get_event_log_with_corrupt_line_returns_invalid_data() {
        // F8: the consolidated helper also covers get_event_log's per-line
        // deserialisation. A single bad line must surface as InvalidData
        // rather than SomeOtherKind so callers can distinguish "corrupted
        // journal" from "missing file".
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join("events.jsonl"), b"not-json\n").unwrap();

        let store = RunStore::new(dir.path()).unwrap();
        let err = store
            .get_event_log()
            .expect_err("corrupt event line must surface as an io::Error");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::InvalidData,
            "corrupt event line must map to InvalidData via serde_to_io; got {:?}",
            err.kind()
        );
    }

    // ----- Cross-cutting safety nets -------------------------------------

    #[test]
    fn as_str_variants_round_trip_through_checkpoint_pipeline() {
        // Property-style test: for every AgentStatus variant, the persisted
        // status string must equal AgentStatus::variant.as_str() exactly,
        // with no whitespace, no case drift, and no truncation. This catches
        // accidental future reverts to Debug-derived strings.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let store = RunStore::new(dir.path()).unwrap();
        store.init_run(run_id, "round-trip property").unwrap();

        let variants = [
            AgentStatus::Ok,
            AgentStatus::Error,
            AgentStatus::Cancelled,
            AgentStatus::TimedOut,
        ];
        let mut seen: HashSet<String> = HashSet::new();

        for variant in &variants {
            let agent_id = uuid::Uuid::now_v7();
            let evt = build_agent_done(run_id, agent_id, variant.clone(), sample_token_usage());
            store.append_event(&evt).unwrap();

            let cp = store.get_checkpoint().unwrap();
            let cached = cp
                .agent_results
                .get(&agent_id)
                .expect("entry for {agent_id}");
            assert_eq!(
                cached.status,
                variant.as_str(),
                "{variant:?}.as_str() must round-trip via append_event→update_from_event"
            );
            // Also confirm uniqueness is preserved on disk.
            assert!(
                seen.insert(cached.status.clone()),
                "duplicate status {cached_status:?} persisted for {variant:?}",
                cached_status = cached.status
            );
        }
    }
}
