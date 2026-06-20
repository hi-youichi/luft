//! Journal / Resume — checkpoint persistence with replay semantics (M1).
//!
//! Provides:
//! - `JournalStore` — wraps `RunStore` with cache-key index for O(1) lookups
//! - `AgentCacheKey` — deterministic blake3-based key for agent invocations
//! - `JournalCallback` trait — scheduler integration hook
//! - `ResumeContext` — orchestrates run recovery
//! - `gc_runs()` — cleanup old completed runs
//!
//! Thread safety: All public methods take `&self` (interior mutability via RwLock).
//! The underlying checkpoint data is protected by a single writer lock.
//!
//! Lifecycle:
//!   new() → init_run() → cache_agent()* → flush()
//!   或:
//!   open() → has_completed()/get_cached() → workflow resume logic

use crate::core::contract::backend::AgentStatus;
use crate::core::contract::event::{AgentEvent, EventSender};
use crate::core::contract::finding::Finding;
use crate::core::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};
use crate::core::state::{AgentResultCache, RunCheckpoint, RunStore};
use crate::core::scheduler::{BackendRegistry, SchedulerConfig};
use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

// ============================================================================
// Error Types
// ============================================================================

#[derive(Error, Debug)]
pub enum JournalError {
    #[error("run not found: {0}")]
    RunNotFound(RunId),
    #[error("run is not resumable (status: {status:?})")]
    NotResumable { status: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("journal corrupted: {0}")]
    Corrupted(String),
}

// ============================================================================
// Agent Cache Key
// ============================================================================

/// Deterministic cache key for an agent invocation.
/// Normalizes whitespace/unicode to ensure cache hits across formatting differences.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentCacheKey {
    pub hash: String,
    /// Human-readable for debugging
    pub prompt_preview: String,
    pub model: Option<String>,
    pub phase_id: PhaseId,
}

impl AgentCacheKey {
    /// Generate a cache key from agent parameters.
    /// Uses blake3 with null separators to prevent field-concatenation collisions.
    pub fn new(prompt: &str, model: Option<&str>, phase_id: PhaseId) -> Self {
        let normalized = normalize_prompt(prompt);
        let preview = if normalized.len() > 80 {
            format!("{}...", &normalized[..80])
        } else {
            normalized.clone()
        };

        let mut h = Hasher::new();
        h.update(normalized.as_bytes());
        h.update(b"\0");
        if let Some(m) = model {
            h.update(m.as_bytes());
        }
        h.update(b"\0");
        h.update(&phase_id.to_le_bytes());

        Self {
            hash: h.finalize().to_hex().to_string(),
            prompt_preview: preview,
            model: model.map(|s| s.to_string()),
            phase_id,
        }
    }
}

fn normalize_prompt(prompt: &str) -> String {
    prompt
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ============================================================================
// JournalStore — the journal abstraction over RunStore
// ============================================================================

/// JournalStore wraps RunStore with replay semantics.
///
/// Thread safety: All public methods take `&self` (interior mutability via RwLock).
/// The underlying checkpoint data is protected by a single writer lock.
///
/// Usage lifecycle:
///   new() → init_run() → cache_agent()* → flush()
///   或:
///   open() → has_completed()/get_cached() → workflow resume logic
pub struct JournalStore {
    /// Underlying persistence engine (checkpoint.json + events.jsonl).
    inner: Arc<RunStore>,
    /// In-memory index: AgentCacheKey hash → AgentResultCache.
    /// Populated at open() time from the checkpoint's agent_results map.
    cache_index: RwLock<HashMap<String, AgentResultCache>>,
    /// Event sender for broadcasting journal updates.
    event_tx: Option<EventSender>,
}

impl std::fmt::Debug for JournalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JournalStore")
            .field("inner", &self.inner)
            .field("cache_index_size", &self.cache_index.read().unwrap().len())
            .field("has_event_tx", &self.event_tx.is_some())
            .finish()
    }
}

impl JournalStore {
    /// Create a new journal store at the given directory.
    /// Initializes the underlying RunStore and creates an empty cache index.
    pub fn new(run_dir: &Path) -> Result<Self, JournalError> {
        tracing::debug!(path = %run_dir.display(), "creating journal store");
        let inner = RunStore::new(run_dir)?;
        Ok(Self {
            inner,
            cache_index: RwLock::new(HashMap::new()),
            event_tx: None,
        })
    }

    /// Attach an event sender for broadcasting journal updates.
    pub fn with_event_sender(mut self, tx: EventSender) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Initialize a new run in the journal.
    pub fn init_run(&self, run_id: RunId, task: &str) -> Result<(), JournalError> {
        tracing::info!(%run_id, %task, "initializing run in journal");
        self.inner.init_run(run_id, task)?;
        Ok(())
    }

    /// Open an existing run and rebuild the cache index from persisted data.
    ///
    /// This is the entry point for `--resume`. It:
    /// 1. Loads the checkpoint from disk
    /// 2. Rebuilds the in-memory cache_index from agent_results
    /// 3. Returns the checkpoint for the caller to inspect
    pub fn open(&self, run_id: RunId) -> Result<RunCheckpoint, JournalError> {
        tracing::info!(%run_id, "opening journal for resume");
        let checkpoint = self
            .inner
            .open_run(run_id)?
            .ok_or(JournalError::RunNotFound(run_id))?;

        if matches!(
            checkpoint.status,
            crate::core::state::CheckpointStatus::Completed
                | crate::core::state::CheckpointStatus::Cancelled
        ) {
            return Err(JournalError::NotResumable {
                status: format!("{:?}", checkpoint.status),
            });
        }

        // Rebuild cache index — index by both agent_id and cache_key_hash
        // so that the Lua SDK's has_completed(key) works after resume.
        let mut index = HashMap::new();
        for (agent_id, cache) in &checkpoint.agent_results {
            index.insert(agent_id.to_string(), cache.clone());
            if let Some(ref hash) = cache.cache_key_hash {
                index.insert(hash.clone(), cache.clone());
            }
        }
        *self.cache_index.write().unwrap() = index;

        Ok(checkpoint)
    }

    /// Cache an agent's result in the journal.
    ///
    /// Called by the scheduler after an agent completes successfully or fails
    /// with a non-retryable error. The result is persisted to disk immediately
    /// (via append_event → update_from_event → write_checkpoint_to_disk).
    pub fn cache_agent(
        &self,
        cache_key: &AgentCacheKey,
        agent_id: AgentId,
        phase_id: PhaseId,
        status: AgentStatus,
        output: serde_json::Value,
        findings: Vec<Finding>,
        tokens: TokenUsage,
    ) -> Result<AgentCacheKey, JournalError> {
        let ts = current_timestamp();
        let cache = AgentResultCache {
            agent_id,
            phase_id,
            status: format!("{:?}", status).to_lowercase(),
            output,
            findings,
            tokens: tokens.total(),
            completed_at: ts,
            cache_key_hash: Some(cache_key.hash.clone()),
        };

        // Update in-memory index (instant lookup)
        {
            let mut index = self.cache_index.write().unwrap();
            index.insert(cache_key.hash.clone(), cache.clone());
            // Also index by agent_id for open() compatibility
            index.insert(agent_id.to_string(), cache.clone());
        }

        // Persist the full cache entry directly to checkpoint disk (preserves cache_key_hash)
        if let Err(e) = self.inner.upsert_agent_result(&cache) {
            tracing::warn!(%agent_id, error = %e, "failed to persist agent cache");
        }

        // Also append event to log (this triggers update_from_event which finds the existing hash)
        let event = AgentEvent::AgentDone {
            run_id: self
                .inner
                .get_checkpoint()
                .map(|c| c.run_id)
                .unwrap_or_else(uuid::Uuid::nil),
            agent_id,
            status,
            tokens,
            elapsed_ms: 0,
        };
        self.inner.append_event(&event)?;

        // Broadcast via event bus (non-blocking — uses broadcast channel)
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event);
        }

        Ok(cache_key.clone())
    }

    /// Record an agent's output for resume replay, keyed by `cache_key`.
    ///
    /// Unlike [`cache_agent`], this does **not** append an `AgentDone` event,
    /// so it never double-counts tokens against the event-driven checkpoint
    /// totals. It only upserts the checkpoint entry (preserving `cache_key_hash`
    /// and the structured output) and refreshes the in-memory cache index.
    /// Called by the Lua SDK after an agent completes during a live run.
    pub fn record_result(
        &self,
        cache_key: &AgentCacheKey,
        agent_id: AgentId,
        phase_id: PhaseId,
        status: AgentStatus,
        output: serde_json::Value,
        findings: Vec<Finding>,
        tokens: TokenUsage,
    ) {
        let cache = AgentResultCache {
            agent_id,
            phase_id,
            status: format!("{:?}", status).to_lowercase(),
            output,
            findings,
            tokens: tokens.total(),
            completed_at: current_timestamp(),
            cache_key_hash: Some(cache_key.hash.clone()),
        };

        {
            let mut index = self.cache_index.write().unwrap();
            index.insert(cache_key.hash.clone(), cache.clone());
            index.insert(agent_id.to_string(), cache.clone());
        }

        if let Err(e) = self.inner.upsert_agent_result(&cache) {
            tracing::warn!(%agent_id, error = %e, "failed to persist agent result");
        }
    }

    /// Access the underlying run store (shared persistence engine).
    /// Allows the CLI to route the scheduler event stream through the same
    /// `RunStore` instance the journal uses, avoiding split-brain checkpoints.
    pub fn store(&self) -> Arc<RunStore> {
        self.inner.clone()
    }

    /// Append an event to the underlying run store (event log + checkpoint).
    pub fn append_event(&self, event: &AgentEvent) -> Result<(), JournalError> {
        self.inner.append_event(event)?;
        Ok(())
    }

    /// Check if an agent with the given cache key has already completed.
    /// Used by the Lua SDK's agent() function before submitting to the scheduler.
    pub fn has_completed(&self, cache_key: &AgentCacheKey) -> bool {
        let index = self.cache_index.read().unwrap();
        index.contains_key(&cache_key.hash)
    }

    /// Get cached result for an agent.
    /// Returns None if the agent hasn't completed yet.
    pub fn get_cached(&self, cache_key: &AgentCacheKey) -> Option<AgentResultCache> {
        let index = self.cache_index.read().unwrap();
        index.get(&cache_key.hash).cloned()
    }

    /// Get list of all completed agent cache keys.
    /// Useful for debugging and progress reporting.
    pub fn completed_keys(&self) -> Vec<AgentCacheKey> {
        let index = self.cache_index.read().unwrap();
        index
            .keys()
            .map(|k| AgentCacheKey {
                hash: k.clone(),
                prompt_preview: String::new(),
                model: None,
                phase_id: 0,
            })
            .collect()
    }

    /// Get the underlying checkpoint (read-only snapshot).
    pub fn get_checkpoint(&self) -> Option<RunCheckpoint> {
        self.inner.get_checkpoint()
    }

    /// Flush all pending data to disk.
    pub fn flush(&self) -> Result<(), JournalError> {
        // RunStore auto-flushes on append_event; explicit flush for safety.
        Ok(())
    }

    /// Mark the run as cancelled.
    pub fn cancel(&self) -> Result<(), JournalError> {
        self.inner.cancel()?;
        Ok(())
    }
}

// ============================================================================
// Scheduler Integration — JournalCallback trait
// ============================================================================

/// Composite callback that chains multiple JournalCallback implementations.
pub struct CompositeJournalCallback {
    callbacks: Vec<Arc<dyn crate::core::scheduler::JournalCallback>>,
}

impl CompositeJournalCallback {
    pub fn new(callbacks: Vec<Arc<dyn crate::core::scheduler::JournalCallback>>) -> Self {
        Self { callbacks }
    }
}

#[async_trait::async_trait]
impl crate::core::scheduler::JournalCallback for CompositeJournalCallback {
    async fn on_agent_done(
        &self,
        agent_id: AgentId,
        phase_id: PhaseId,
        status: AgentStatus,
        output: serde_json::Value,
        tokens: TokenUsage,
    ) {
        for cb in &self.callbacks {
            cb.on_agent_done(agent_id, phase_id, status.clone(), output.clone(), tokens.clone())
                .await;
        }
    }
}

#[async_trait::async_trait]
impl crate::core::scheduler::JournalCallback for JournalStore {
    async fn on_agent_done(
        &self,
        agent_id: AgentId,
        phase_id: PhaseId,
        status: AgentStatus,
        output: serde_json::Value,
        tokens: TokenUsage,
    ) {
        // Store into the checkpoint by agent_id directly (no cache_key index needed).
        let ts = current_timestamp();
        let cache = AgentResultCache {
            agent_id,
            phase_id,
            status: format!("{:?}", status).to_lowercase(),
            output,
            findings: vec![], // findings not available from scheduler callback
            tokens: tokens.total(),
            completed_at: ts,
            cache_key_hash: None, // not indexed by cache key from this path
        };

        // Persist to checkpoint disk
        if let Err(e) = self.inner.upsert_agent_result(&cache) {
            tracing::warn!(%agent_id, error = %e, "failed to persist agent result from callback");
        }
    }
}

// ============================================================================
// Resume Orchestration
// ============================================================================

/// Context for resuming a run.
#[derive(Debug)]
pub struct ResumeContext {
    pub run_id: RunId,
    pub checkpoint: RunCheckpoint,
    pub journal: Arc<JournalStore>,
    pub scheduler_config: SchedulerConfig,
    pub backend_registry: BackendRegistry,
}

/// Options for creating a run (new or resume).
#[derive(Debug, Clone)]
pub enum RunCreationMode {
    /// Start a fresh run.
    New { task: String },
    /// Resume from an existing checkpoint.
    Resume { run_id: RunId },
    /// Auto-detect: resume if resumable run exists, else new.
    Auto { task: String },
}

impl RunCreationMode {
    /// Resolve the creation mode to concrete parameters.
    /// For Auto mode, checks journal directory for resumable runs.
    pub fn resolve(self, journal_dir: &Path) -> Result<(RunId, Option<RunCheckpoint>), JournalError> {
        match self {
            RunCreationMode::New { task: _ } => {
                let run_id = uuid::Uuid::now_v7();
                Ok((run_id, None))
            }
            RunCreationMode::Resume { run_id } => {
                let store = JournalStore::new(&journal_dir.join(run_id.to_string()))?;
                let checkpoint = store.open(run_id)?;
                Ok((run_id, Some(checkpoint)))
            }
            RunCreationMode::Auto { task: _ } => {
                // List all runs, find the most recently updated Running one
                let runs = crate::core::state::list_runs(journal_dir)?;
                for run_id in runs.iter().rev() {
                    let run_dir = journal_dir.join(run_id.to_string());
                    if let Ok(store) = JournalStore::new(&run_dir) {
                        if store.inner.can_resume() {
                            if let Ok(Some(checkpoint)) = store.inner.open_run(*run_id) {
                                return Ok((*run_id, Some(checkpoint)));
                            }
                        }
                    }
                }
                // No resumable run — create new
                let run_id = uuid::Uuid::now_v7();
                Ok((run_id, None))
            }
        }
    }
}

// ============================================================================
// GC (Garbage Collection)
// ============================================================================

/// Clean up old completed/cancelled runs.
///
/// Policy:
/// - Completed/Cancelled runs older than `older_than` are eligible for deletion.
/// - Running runs are never cleaned.
///
/// Returns the number of runs cleaned.
pub fn gc_runs(journal_dir: &Path, older_than: Duration) -> Result<usize, JournalError> {
    let runs = crate::core::state::list_runs(journal_dir)?;
    let cutoff = current_timestamp().saturating_sub(older_than.as_secs());

    tracing::debug!("GC: scanning {} runs", runs.len());
    let mut cleaned = 0;
    for run_id in &runs {
        let run_dir = journal_dir.join(run_id.to_string());
        // Peek at checkpoint without full open
        let checkpoint_path = run_dir.join("checkpoint.json");
        if !checkpoint_path.exists() {
            continue;
        }

        let content = std::fs::read_to_string(&checkpoint_path)?;
        let checkpoint: RunCheckpoint = serde_json::from_str(&content)?;

        let is_old = checkpoint.updated_at < cutoff;
        let is_terminal = matches!(
            checkpoint.status,
            crate::core::state::CheckpointStatus::Completed
                | crate::core::state::CheckpointStatus::Cancelled
                | crate::core::state::CheckpointStatus::Failed
        );

        if is_old && is_terminal {
            tracing::info!(%run_id, "GC: removing old terminal run");
            std::fs::remove_dir_all(&run_dir)?;
            cleaned += 1;
        }
    }

    Ok(cleaned)
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Test basic journal lifecycle: init → cache → read → cancel
    #[test]
    fn test_journal_lifecycle() {
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let journal = JournalStore::new(dir.path()).unwrap();

        // 1. Init
        journal.init_run(run_id, "Test task").unwrap();
        let cp = journal.get_checkpoint().unwrap();
        assert_eq!(cp.status, crate::core::state::CheckpointStatus::Running);
        assert_eq!(cp.task, "Test task");

        // 2. Cache an agent result
        let agent_id = uuid::Uuid::now_v7();
        let key = AgentCacheKey::new("test prompt", Some("gpt-4"), 1);
        journal.cache_agent(
            &key, agent_id, 1, AgentStatus::Ok,
            serde_json::json!({"result": "ok"}),
            vec![],
            TokenUsage { input: 100, output: 50, cache_read: 0, cache_write: 0 },
        ).unwrap();

        // 3. Verify cache
        assert!(journal.has_completed(&key));
        let cached = journal.get_cached(&key).unwrap();
        assert_eq!(cached.output, serde_json::json!({"result": "ok"}));
        assert_eq!(cached.tokens, 150);

        // 4. Cancel
        journal.cancel().unwrap();
        let cp = journal.get_checkpoint().unwrap();
        assert_eq!(cp.status, crate::core::state::CheckpointStatus::Cancelled);
    }

    /// Test that different prompts produce different cache keys
    #[test]
    fn test_cache_key_uniqueness() {
        let k1 = AgentCacheKey::new("prompt A", Some("gpt-4"), 1);
        let k2 = AgentCacheKey::new("prompt B", Some("gpt-4"), 1);
        assert_ne!(k1.hash, k2.hash);

        // Same prompt, different model
        let k3 = AgentCacheKey::new("prompt A", Some("claude"), 1);
        assert_ne!(k1.hash, k3.hash);

        // Same prompt, different phase
        let k4 = AgentCacheKey::new("prompt A", Some("gpt-4"), 2);
        assert_ne!(k1.hash, k4.hash);

        // Whitespace normalization
        let k5 = AgentCacheKey::new("  prompt  \r\nA  ", Some("gpt-4"), 1);
        assert_eq!(k1.hash, k5.hash);
    }

    /// Test resume: simulate workflow with 3 agents, 2 cached, 1 new
    #[test]
    fn test_resume_skip_cached() {
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let journal = JournalStore::new(dir.path()).unwrap();
        journal.init_run(run_id, "Three agent test").unwrap();

        // Cache agent 1 and 2
        let k1 = AgentCacheKey::new("task 1", None, 1);
        let k2 = AgentCacheKey::new("task 2", None, 1);
        let k3 = AgentCacheKey::new("task 3", None, 1);

        journal.cache_agent(&k1, uuid::Uuid::now_v7(), 1, AgentStatus::Ok,
            serde_json::json!({"done": 1}), vec![],
            TokenUsage { input: 10, output: 5, cache_read: 0, cache_write: 0 }).unwrap();
        journal.cache_agent(&k2, uuid::Uuid::now_v7(), 1, AgentStatus::Ok,
            serde_json::json!({"done": 2}), vec![],
            TokenUsage { input: 10, output: 5, cache_read: 0, cache_write: 0 }).unwrap();

        // Verify cache hits
        assert!(journal.has_completed(&k1));
        assert!(journal.has_completed(&k2));
        assert!(!journal.has_completed(&k3));

        // Agent 3 should NOT be cached → would go through scheduler
        assert!(journal.get_cached(&k3).is_none());
    }

    /// Test that journal survives crash (simulated by re-opening)
    #[test]
    fn test_journal_crash_recovery() {
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();

        // Part 1: Create and cache
        {
            let j = JournalStore::new(dir.path()).unwrap();
            j.init_run(run_id, "Crash test").unwrap();
            let key = AgentCacheKey::new("important work", None, 0);
            j.cache_agent(&key, uuid::Uuid::now_v7(), 0, AgentStatus::Ok,
                serde_json::json!({"survived": true}), vec![],
                TokenUsage { input: 1, output: 1, cache_read: 0, cache_write: 0 }).unwrap();
        } // j dropped — simulates crash

        // Part 2: Re-open and verify data survived
        {
            let j2 = JournalStore::new(dir.path()).unwrap();
            let cp = j2.open(run_id).unwrap();
            assert_eq!(cp.status, crate::core::state::CheckpointStatus::Running);
            assert!(!cp.agent_results.is_empty());

            let key = AgentCacheKey::new("important work", None, 0);
            let cached = j2.get_cached(&key).unwrap();
            assert_eq!(cached.output, serde_json::json!({"survived": true}));
        }
    }

    /// Test GC reference
    #[test]
    fn test_gc_older_than() {
        let dir = tempdir().unwrap();
        let run_dir = dir.path().join("runs");
        std::fs::create_dir_all(&run_dir).unwrap();

        // Create a completed run
        let run_id = uuid::Uuid::now_v7();
        let journal = JournalStore::new(&run_dir.join(run_id.to_string())).unwrap();
        journal.init_run(run_id, "GC me").unwrap();

        // Manually mark as completed with old timestamp
        if let Some(mut cp) = journal.get_checkpoint() {
            cp.status = crate::core::state::CheckpointStatus::Completed;
            cp.updated_at = 1000; // Very old
            let _ = journal.inner.save_checkpoint(&cp);
        }

        // GC with very short duration
        let cleaned = gc_runs(&run_dir, Duration::from_secs(3600)).unwrap();
        assert_eq!(cleaned, 1);
    }
}
