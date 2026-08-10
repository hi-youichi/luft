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

use crate::contract::finding::Finding;
use crate::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};
use crate::scheduler::{BackendRegistry, SchedulerConfig};
use crate::session::{resolve_session, restore_session};
use crate::state::{AgentResultCache, AgentSessionCheckpoint, CheckpointBackend, RunCheckpoint};
use blake3::Hasher;
use chrono::Utc;
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
    #[error("backend error: {0}")]
    Backend(String),
}

fn map_anyhow(e: anyhow::Error) -> JournalError {
    JournalError::Backend(e.to_string())
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
    pub phase_id: PhaseId,
}

impl AgentCacheKey {
    /// Generate a cache key from agent parameters.
    /// Uses blake3 with null separators to prevent field-concatenation collisions.
    pub fn new(prompt: &str, phase_id: PhaseId) -> Self {
        let normalized = normalize_prompt(prompt);
        let preview = if normalized.chars().count() > 80 {
            format!("{}...", normalized.chars().take(80).collect::<String>())
        } else {
            normalized.clone()
        };

        let mut h = Hasher::new();
        h.update(normalized.as_bytes());
        h.update(b"\0");
        h.update(&phase_id.to_le_bytes());

        Self {
            hash: h.finalize().to_hex().to_string(),
            prompt_preview: preview,
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
    /// Underlying persistence engine (SQLite-backed CheckpointBackend).
    inner: Arc<dyn CheckpointBackend>,
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
    /// Create a new journal store backed by the given `CheckpointBackend`.
    pub fn with_backend(backend: Arc<dyn CheckpointBackend>) -> Self {
        tracing::debug!(backend = ?backend, "creating journal store with backend");
        Self {
            inner: backend,
            cache_index: RwLock::new(HashMap::new()),
            event_tx: None,
        }
    }

    /// Create a new journal store at the given directory.
    /// Convenience constructor — requires the caller to provide a backend factory.
    /// Deprecated: prefer `with_backend`.
    #[deprecated(note = "use JournalStore::with_backend instead")]
    pub fn new(_run_dir: &Path) -> Result<Self, JournalError> {
        Err(JournalError::Corrupted(
            "JournalStore::new(path) is no longer supported. Use JournalStore::with_backend(backend).".into()
        ))
    }

    /// Initialize a new run in the journal.
    pub fn init_run(&self, run_id: RunId, task: &str, run_dir: &str) -> Result<(), JournalError> {
        tracing::info!(%run_id, %task, "initializing run in journal");
        self.inner.init_run(run_id, task, run_dir).map_err(map_anyhow)?;
        Ok(())
    }

    /// Initialize a new run with declarative workflow metadata.
    pub fn init_run_with_meta(
        &self,
        run_id: RunId,
        task: &str,
        run_dir: &str,
        workflow_meta: serde_json::Value,
    ) -> Result<(), JournalError> {
        tracing::info!(
            %run_id, %task,
            "initializing run in journal with meta"
        );
        self.inner.init_run_with_meta(run_id, task, run_dir, workflow_meta).map_err(map_anyhow)?;
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
            .open_run(run_id).map_err(map_anyhow)?
            .ok_or(JournalError::RunNotFound(run_id))?;

        if matches!(
            checkpoint.status,
            crate::state::CheckpointStatus::Completed
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
        self.inner.append_event(&event).map_err(map_anyhow)?;

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

    /// Persist the session id returned by a backend for later diagnostics and
    /// same-run resume. The id is opaque to the journal; backend-specific
    /// conversation state is not serialized here.
    pub fn record_session(
        &self,
        agent_id: AgentId,
        session_id: String,
        status: &str,
        resumable: bool,
    ) {
        let backend_id = crate::contract::current_backend().map(|backend| backend.id);
        let protocol_session_id = backend_id
            .as_deref()
            .and_then(|backend| resolve_session(&session_id, backend))
            .map(|record| record.protocol_session_id)
            .or_else(|| Some(session_id.clone()));
        let session = AgentSessionCheckpoint {
            agent_id,
            backend_id,
            protocol_session_id,
            session_id,
            status: status.to_string(),
            updated_at: current_timestamp(),
            resumable,
        };
        if let Err(e) = self.inner.upsert_agent_session(&session) {
            tracing::warn!(%agent_id, error = %e, "failed to persist agent session");
        }
    }

    /// Return the persisted session metadata for an agent, if any.
    pub fn get_session(&self, agent_id: AgentId) -> Option<AgentSessionCheckpoint> {
        let session = self
            .inner
            .get_checkpoint()
            .and_then(|checkpoint| checkpoint.agent_sessions.get(&agent_id).cloned());
        if let Some(ref session) = session {
            if let (Some(backend_id), Some(protocol_id)) =
                (session.backend_id.as_deref(), session.protocol_session_id.as_deref())
            {
                restore_session(&session.session_id, backend_id, protocol_id);
            }
        }
        session
    }

    /// Access the underlying run store (shared persistence engine).
    /// Allows the CLI to route the scheduler event stream through the same
    /// `RunStore` instance the journal uses, avoiding split-brain checkpoints.
    pub fn store(&self) -> Arc<dyn CheckpointBackend> {
        self.inner.clone()
    }

    /// Append an event to the underlying run store (event log + checkpoint).
    pub fn append_event(&self, event: &AgentEvent) -> Result<(), JournalError> {
        self.inner.append_event(event).map_err(map_anyhow)?;
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
        self.inner.cancel().map_err(map_anyhow)?;
        Ok(())
    }

    /// Reset checkpoint status to `Running`. Used when resuming a
    /// failed/cancelled run.
    pub fn reset_status_to_running(&self) -> Result<(), JournalError> {
        self.inner.reset_status_to_running().map_err(map_anyhow)?;
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
        let ts = current_timestamp();

        // Preserve cache_key_hash and other enriched fields from a prior
        // cache_agent() / record_result() call.  Without this, the scheduler
        // callback would overwrite the hash with None, causing the agent to be
        // re-executed on resume even though it already completed.
        let existing = {
            let index = self.cache_index.read().unwrap();
            index.get(&agent_id.to_string()).cloned()
        };

        let cache = AgentResultCache {
            agent_id,
            phase_id: existing.as_ref().map(|c| c.phase_id).unwrap_or(phase_id),
            status: status.as_str().to_string(),
            output: existing
                .as_ref()
                .filter(|c| !c.output.is_null())
                .map(|c| c.output.clone())
                .unwrap_or(output),
            findings: existing
                .as_ref()
                .filter(|c| !c.findings.is_empty())
                .map(|c| c.findings.clone())
                .unwrap_or_default(),
            tokens: tokens.total(),
            completed_at: ts,
            cache_key_hash: existing.as_ref().and_then(|c| c.cache_key_hash.clone()),
            description: existing.as_ref().and_then(|c| c.description.clone()),
            role: existing.as_ref().and_then(|c| c.role.clone()),
        };

        // Update in-memory index so subsequent on_agent_done calls also see
        // the preserved hash.
        {
            let mut index = self.cache_index.write().unwrap();
            index.insert(agent_id.to_string(), cache.clone());
            if let Some(ref hash) = cache.cache_key_hash {
                index.insert(hash.clone(), cache.clone());
            }
        }

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
    /// `backend_factory` creates a `CheckpointBackend` for a given run directory.
    pub fn resolve(
        self,
        journal_dir: &Path,
        backend_factory: &dyn Fn(&Path) -> Arc<dyn CheckpointBackend>,
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
                let backend = backend_factory(&journal_dir.join(&run_dir_name));
                let store = JournalStore::with_backend(backend);
                let checkpoint = store.open(run_id)?;
                Ok((run_id, Some(checkpoint)))
            }
            RunCreationMode::Auto { task: _ } => {
                let run_dirs = crate::state::list_run_dirs(journal_dir).map_err(map_anyhow)?;
                for dir_name in run_dirs.iter().rev() {
                    let run_dir = journal_dir.join(dir_name);
                    let backend = backend_factory(&run_dir);
                    if let Ok(Some(checkpoint)) = backend.open_run(uuid::Uuid::nil()) {
                        if matches!(checkpoint.status, crate::state::CheckpointStatus::Running)
                            || matches!(checkpoint.status, crate::state::CheckpointStatus::Failed)
                            || matches!(checkpoint.status, crate::state::CheckpointStatus::Cancelled)
                        {
                            let run_id = checkpoint.run_id;
                            return Ok((run_id, Some(checkpoint)));
                        }
                    }
                }
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
    let run_dirs = crate::state::list_run_dirs(journal_dir).map_err(map_anyhow)?;
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
// Tests — moved to luft-storage (SqliteCheckpointBackend integration tests)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_key_uniqueness() {
        let k1 = AgentCacheKey::new("prompt A", 1);
        let k2 = AgentCacheKey::new("prompt B", 1);
        assert_ne!(k1.hash, k2.hash);

        // Same prompt, different phase
        let k4 = AgentCacheKey::new("prompt A", 2);
        assert_ne!(k1.hash, k4.hash);

        // Whitespace normalization
        let k5 = AgentCacheKey::new("  prompt  \r\nA  ", 1);
        assert_eq!(k1.hash, k5.hash);
    }
}
