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

use crate::contract::backend::AgentStatus;
use crate::contract::event::{AgentEvent, EventSender};
use chrono::Utc;
use crate::contract::finding::Finding;
use crate::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};
use crate::scheduler::{BackendRegistry, SchedulerConfig};
use crate::state::{AgentResultCache, RunCheckpoint, RunStore};
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
        let preview = if normalized.chars().count() > 80 {
            format!("{}...", normalized.chars().take(80).collect::<String>())
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

    /// Initialize a new run with declarative workflow metadata.
    pub fn init_run_with_meta(
        &self,
        run_id: RunId,
        task: &str,
        workflow_meta: serde_json::Value,
    ) -> Result<(), JournalError> {
        tracing::info!(
            %run_id, %task,
            "initializing run in journal with meta"
        );
        self.inner.init_run_with_meta(run_id, task, workflow_meta)?;
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
            crate::state::CheckpointStatus::Completed | crate::state::CheckpointStatus::Cancelled
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
    #[allow(clippy::too_many_arguments)]
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
            status: status.as_str().to_string(),
            output,
            findings,
            tokens: tokens.total(),
            completed_at: ts,
            cache_key_hash: Some(cache_key.hash.clone()),
            description: None,
            role: None,
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
            name: None,
            agent_seq: 0,
            output: serde_json::Value::Null,
            findings: Vec::new(),
            prompt: String::new(),
            retry_count: 0,
            ts: Utc::now(),
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
    #[allow(clippy::too_many_arguments)]
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
            status: status.as_str().to_string(),
            output,
            findings,
            tokens: tokens.total(),
            completed_at: current_timestamp(),
            cache_key_hash: Some(cache_key.hash.clone()),
            description: None,
            role: None,
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
    callbacks: Vec<Arc<dyn crate::scheduler::JournalCallback>>,
}

impl CompositeJournalCallback {
    pub fn new(callbacks: Vec<Arc<dyn crate::scheduler::JournalCallback>>) -> Self {
        Self { callbacks }
    }
}

#[async_trait::async_trait]
impl crate::scheduler::JournalCallback for CompositeJournalCallback {
    async fn on_agent_done(
        &self,
        agent_id: AgentId,
        phase_id: PhaseId,
        status: AgentStatus,
        output: serde_json::Value,
        tokens: TokenUsage,
    ) {
        for cb in &self.callbacks {
            cb.on_agent_done(agent_id, phase_id, status.clone(), output.clone(), tokens)
                .await;
        }
    }
}

#[async_trait::async_trait]
impl crate::scheduler::JournalCallback for JournalStore {
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
            status: status.as_str().to_string(),
            output,
            findings: vec![], // findings not available from scheduler callback
            tokens: tokens.total(),
            completed_at: ts,
            cache_key_hash: None, // not indexed by cache key from this path
            description: None,
            role: None,
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
    Resume { run_id: RunId, run_dir_name: String },
    /// Auto-detect: resume if resumable run exists, else new.
    Auto { task: String },
}

impl RunCreationMode {
    /// Resolve the creation mode to concrete parameters.
    /// For Auto mode, checks journal directory for resumable runs.
    pub fn resolve(
        self,
        journal_dir: &Path,
    ) -> Result<(RunId, Option<RunCheckpoint>), JournalError> {
        match self {
            RunCreationMode::New { task: _ } => {
                let run_id = uuid::Uuid::now_v7();
                Ok((run_id, None))
            }
            RunCreationMode::Resume {
                run_id,
                run_dir_name,
            } => {
                let store = JournalStore::new(&journal_dir.join(&run_dir_name))?;
                let checkpoint = store.open(run_id)?;
                Ok((run_id, Some(checkpoint)))
            }
            RunCreationMode::Auto { task: _ } => {
                // List all run dirs, find the most recent Running checkpoint
                let run_dirs = crate::state::list_runs(journal_dir)?;
                for dir_name in run_dirs.iter().rev() {
                    let checkpoint_path = journal_dir.join(dir_name).join("checkpoint.json");
                    if let Ok(content) = std::fs::read_to_string(&checkpoint_path) {
                        if let Ok(checkpoint) = serde_json::from_str::<RunCheckpoint>(&content) {
                            if matches!(checkpoint.status, crate::state::CheckpointStatus::Running)
                            {
                                let run_id = checkpoint.run_id;
                                return Ok((run_id, Some(checkpoint)));
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
    let run_dirs = crate::state::list_runs(journal_dir)?;
    let cutoff = current_timestamp().saturating_sub(older_than.as_secs());

    tracing::debug!("GC: scanning {} runs", run_dirs.len());
    let mut cleaned = 0;
    for dir_name in &run_dirs {
        let run_dir = journal_dir.join(dir_name);
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
            crate::state::CheckpointStatus::Completed
                | crate::state::CheckpointStatus::Cancelled
                | crate::state::CheckpointStatus::Failed
        );

        if is_old && is_terminal {
            tracing::info!(dir = %dir_name, "GC: removing old terminal run");
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
        assert_eq!(cp.status, crate::state::CheckpointStatus::Running);
        assert_eq!(cp.task, "Test task");

        // 2. Cache an agent result
        let agent_id = uuid::Uuid::now_v7();
        let key = AgentCacheKey::new("test prompt", Some("gpt-4"), 1);
        journal
            .cache_agent(
                &key,
                agent_id,
                1,
                AgentStatus::Ok,
                serde_json::json!({"result": "ok"}),
                vec![],
                TokenUsage {
                    input: 100,
                    output: 50,
                    cache_read: 0,
                    cache_write: 0,
                },
            )
            .unwrap();

        // 3. Verify cache
        assert!(journal.has_completed(&key));
        let cached = journal.get_cached(&key).unwrap();
        assert_eq!(cached.output, serde_json::json!({"result": "ok"}));
        assert_eq!(cached.tokens, 150);

        // 4. Cancel
        journal.cancel().unwrap();
        let cp = journal.get_checkpoint().unwrap();
        assert_eq!(cp.status, crate::state::CheckpointStatus::Cancelled);
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

        journal
            .cache_agent(
                &k1,
                uuid::Uuid::now_v7(),
                1,
                AgentStatus::Ok,
                serde_json::json!({"done": 1}),
                vec![],
                TokenUsage {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                },
            )
            .unwrap();
        journal
            .cache_agent(
                &k2,
                uuid::Uuid::now_v7(),
                1,
                AgentStatus::Ok,
                serde_json::json!({"done": 2}),
                vec![],
                TokenUsage {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                },
            )
            .unwrap();

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
            j.cache_agent(
                &key,
                uuid::Uuid::now_v7(),
                0,
                AgentStatus::Ok,
                serde_json::json!({"survived": true}),
                vec![],
                TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                },
            )
            .unwrap();
        } // j dropped — simulates crash

        // Part 2: Re-open and verify data survived
        {
            let j2 = JournalStore::new(dir.path()).unwrap();
            let cp = j2.open(run_id).unwrap();
            assert_eq!(cp.status, crate::state::CheckpointStatus::Running);
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
            cp.status = crate::state::CheckpointStatus::Completed;
            cp.updated_at = 1000; // Very old
            let _ = journal.inner.save_checkpoint(&cp);
        }

        // GC with very short duration
        let cleaned = gc_runs(&run_dir, Duration::from_secs(3600)).unwrap();
        assert_eq!(cleaned, 1);
    }

    // ----------------------------------------------------------------------
    // Tests for the F5 contract — AgentResultCache.status persistence.
    //
    // cache_agent, record_result, and the JournalCallback impl for
    // JournalStore all persist AgentResultCache.status. Before F5 the value
    // was derived from `format!("{:?}", status).to_lowercase()`, which
    // silently mis-mapped TimedOut → "timedout" (no underscore). The
    // implementations must now use `AgentStatus::as_str()` and produce
    // snake_case strings that match the canonical on-disk mapping.
    // ----------------------------------------------------------------------

    fn read_checkpoint_status_for(run_dir: &std::path::Path, agent_id: AgentId) -> Option<String> {
        let cp_path = run_dir.join("checkpoint.json");
        let content = std::fs::read_to_string(&cp_path).ok()?;
        let raw: serde_json::Value = serde_json::from_str(&content).ok()?;
        let ar = raw.get("agent_results")?.as_object()?;
        for (_k, v) in ar {
            if v.get("agent_id").and_then(|id| id.as_str()) == Some(&agent_id.to_string()) {
                return v.get("status").and_then(|s| s.as_str()).map(String::from);
            }
        }
        None
    }

    fn sample_token_usage(input: u64, output: u64) -> TokenUsage {
        TokenUsage {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
        }
    }

    #[test]
    fn cache_agent_persists_snake_case_status_for_each_variant() {
        // F5 KEY test for JournalStore::cache_agent: the persisted status
        // MUST equal AgentStatus::as_str() (snake_case), not Debug lowercased.
        // Particularly important for TimedOut which would otherwise round-trip
        // as "timedout" (no underscore) and break cross-process resume.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let journal = JournalStore::new(dir.path()).unwrap();
        journal.init_run(run_id, "cache_agent F5").unwrap();

        let cases: Vec<(AgentStatus, &str)> = vec![
            (AgentStatus::Ok, "ok"),
            (AgentStatus::Error, "error"),
            (AgentStatus::Cancelled, "cancelled"),
            (AgentStatus::TimedOut, "timed_out"),
        ];
        for (status, expected) in &cases {
            let agent_id = uuid::Uuid::now_v7();
            let key = AgentCacheKey::new("prompt", Some("gpt-4"), 1);
            journal
                .cache_agent(
                    &key,
                    agent_id,
                    1,
                    status.clone(),
                    serde_json::json!({"v": 1}),
                    vec![],
                    sample_token_usage(10, 5),
                )
                .unwrap();

            let persisted = read_checkpoint_status_for(dir.path(), agent_id)
                .unwrap_or_else(|| panic!("status missing on disk for {status:?}"));
            assert_eq!(
                persisted, *expected,
                "cache_agent({status:?}) must persist status={expected:?} (snake_case); \
                 got {persisted:?}. Reverting to Debug formatting would yield \"timedout\" \
                 for TimedOut and break the on-disk contract."
            );
        }
    }

    #[test]
    fn cache_agent_timed_out_persists_with_underscore_not_collapsed() {
        // Strongest F5 regression guard for cache_agent: TimedOut MUST persist
        // as "timed_out" with an underscore. The buggy Debug-lowercased path
        // would produce "timedout" and silently corrupt the journal.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let journal = JournalStore::new(dir.path()).unwrap();
        journal.init_run(run_id, "timed-out guard").unwrap();

        let agent_id = uuid::Uuid::now_v7();
        let key = AgentCacheKey::new("p", None, 0);
        journal
            .cache_agent(
                &key,
                agent_id,
                0,
                AgentStatus::TimedOut,
                serde_json::json!(null),
                vec![],
                sample_token_usage(1, 2),
            )
            .unwrap();

        let persisted = read_checkpoint_status_for(dir.path(), agent_id).expect("status on disk");
        assert_eq!(
            persisted, "timed_out",
            "cache_agent(TimedOut) must persist \"timed_out\"; got {persisted:?}"
        );
        assert_ne!(
            persisted, "timedout",
            "cache_agent(TimedOut) must NOT collapse to Debug-lowercased \"timedout\""
        );
    }

    #[test]
    fn record_result_persists_snake_case_status_for_each_variant() {
        // F5 test for JournalStore::record_result: same snake_case contract
        // applies to the non-event-emitting path used by Lua SDK callbacks.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let journal = JournalStore::new(dir.path()).unwrap();
        journal.init_run(run_id, "record_result F5").unwrap();

        let cases: Vec<(AgentStatus, &str)> = vec![
            (AgentStatus::Ok, "ok"),
            (AgentStatus::Error, "error"),
            (AgentStatus::Cancelled, "cancelled"),
            (AgentStatus::TimedOut, "timed_out"),
        ];
        for (status, expected) in &cases {
            let agent_id = uuid::Uuid::now_v7();
            let key = AgentCacheKey::new("p", None, 1);
            journal.record_result(
                &key,
                agent_id,
                1,
                status.clone(),
                serde_json::json!({"r": 1}),
                vec![],
                sample_token_usage(2, 3),
            );

            let persisted = read_checkpoint_status_for(dir.path(), agent_id)
                .unwrap_or_else(|| panic!("status missing on disk for {status:?}"));
            assert_eq!(
                persisted, *expected,
                "record_result({status:?}) must persist status={expected:?}; got {persisted:?}"
            );
        }
    }

    #[test]
    fn record_result_timed_out_persists_with_underscore() {
        // Same regression guard for record_result.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let journal = JournalStore::new(dir.path()).unwrap();
        journal.init_run(run_id, "record_result timed-out").unwrap();

        let agent_id = uuid::Uuid::now_v7();
        let key = AgentCacheKey::new("p", None, 0);
        journal.record_result(
            &key,
            agent_id,
            0,
            AgentStatus::TimedOut,
            serde_json::json!(null),
            vec![],
            sample_token_usage(0, 0),
        );

        let persisted = read_checkpoint_status_for(dir.path(), agent_id).expect("status on disk");
        assert_eq!(persisted, "timed_out");
        assert_ne!(persisted, "timedout");
    }

    #[tokio::test]
    async fn journal_callback_on_agent_done_persists_snake_case_status() {
        // F5 test for the JournalCallback impl on JournalStore. The scheduler
        // calls `on_agent_done` when an agent finishes; the persisted
        // AgentResultCache.status MUST match AgentStatus::as_str() exactly.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let journal = std::sync::Arc::new(JournalStore::new(dir.path()).unwrap());
        journal.init_run(run_id, "callback F5").unwrap();

        let cases: Vec<(AgentStatus, &str)> = vec![
            (AgentStatus::Ok, "ok"),
            (AgentStatus::Error, "error"),
            (AgentStatus::Cancelled, "cancelled"),
            (AgentStatus::TimedOut, "timed_out"),
        ];
        for (status, expected) in &cases {
            let agent_id = uuid::Uuid::now_v7();
            use crate::scheduler::JournalCallback;
            journal
                .on_agent_done(
                    agent_id,
                    1,
                    status.clone(),
                    serde_json::json!({}),
                    sample_token_usage(4, 6),
                )
                .await;

            let persisted = read_checkpoint_status_for(dir.path(), agent_id)
                .unwrap_or_else(|| panic!("status missing on disk for {status:?}"));
            assert_eq!(
                persisted, *expected,
                "JournalCallback::on_agent_done({status:?}) must persist status={expected:?}; \
                 got {persisted:?}"
            );
        }
    }

    #[test]
    fn record_result_then_reopen_uses_snake_case_status() {
        // Snake_case persistence must survive a close+reopen cycle so a
        // resumed process sees the canonical strings (not Debug leftovers).
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let journal = JournalStore::new(dir.path()).unwrap();
        journal.init_run(run_id, "reopen F5").unwrap();

        let agent_id = uuid::Uuid::now_v7();
        let key = AgentCacheKey::new("reopen prompt", Some("gpt-4"), 1);
        journal.record_result(
            &key,
            agent_id,
            1,
            AgentStatus::Cancelled,
            serde_json::json!({"result": "ok"}),
            vec![],
            sample_token_usage(7, 11),
        );
        drop(journal);

        let j2 = JournalStore::new(dir.path()).unwrap();
        let cp = j2.open(run_id).expect("open after drop");
        let cached = cp
            .agent_results
            .get(&agent_id)
            .expect("entry survives reopen");
        assert_eq!(
            cached.status, "cancelled",
            "snake_case status must round-trip through close+reopen"
        );
        assert_eq!(cached.tokens, 18);
    }

    #[test]
    fn cache_agent_persists_snake_case_status_to_event_log() {
        // The AgentDone event itself also travels through the same snake_case
        // contract (via update_from_event → as_str()). Read events.jsonl back
        // and confirm the event log carries the canonical status.
        let dir = tempdir().unwrap();
        let run_id = uuid::Uuid::now_v7();
        let journal = JournalStore::new(dir.path()).unwrap();
        journal.init_run(run_id, "event log F5").unwrap();

        let agent_id = uuid::Uuid::now_v7();
        let key = AgentCacheKey::new("p", None, 1);
        journal
            .cache_agent(
                &key,
                agent_id,
                1,
                AgentStatus::TimedOut,
                serde_json::json!(null),
                vec![],
                sample_token_usage(1, 1),
            )
            .unwrap();

        // The persisted AgentResultCache.status must already be verified by
        // the test above; this test only confirms the event log still parses
        // and carries the AgentDone event with the right status enum.
        let log = journal.store().get_event_log().expect("read events.jsonl");
        let agent_done = log
            .iter()
            .find_map(|e| match e {
                AgentEvent::AgentDone {
                    agent_id: id,
                    status,
                    ..
                } if id == &agent_id => Some(status.clone()),
                _ => None,
            })
            .expect("AgentDone event in log");
        // Status enum round-trip is enforced by serde, but the persisted
        // cache status string (verified above) is the part that the on-disk
        // contract depends on.
        assert!(matches!(agent_done, AgentStatus::TimedOut));
    }
}
