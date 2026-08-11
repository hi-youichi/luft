//! SQLite-backed `CheckpointBackend` implementation.
//!
//! Replaces file-based `checkpoint.json` + `events.jsonl` with a unified
//! SQLite store. All methods are synchronous; async sqlx calls are bridged
//! via `block_in_place` + `Handle::block_on`.

use crate::db::DbPool;
use crate::writer::EventWriter;
use luft_core::contract::event::AgentEvent;
use luft_core::contract::finding::Finding;
use luft_core::contract::ids::{AgentId, RunId};
use luft_core::state::{
    AgentResultCache, AgentSessionCheckpoint, CheckpointBackend, CheckpointStatus, PhaseSummary,
    RunCheckpoint,
};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::RwLock;

/// SQLite-backed checkpoint backend.
///
/// Wraps an `EventWriter` for structured event-to-SQL translation and adds
/// checkpoint table management on top. Maintains an in-memory checkpoint
/// cache for O(1) hot-path reads.
#[derive(Debug)]
pub struct SqliteCheckpointBackend {
    pool: DbPool,
    run_id: RunId,
    /// In-memory checkpoint cache (hot-path read).
    checkpoint: RwLock<Option<RunCheckpoint>>,
}

impl SqliteCheckpointBackend {
    /// Create a new backend for the given run.
    pub fn new(pool: DbPool, run_id: RunId) -> Self {
        Self {
            pool,
            run_id,
            checkpoint: RwLock::new(None),
        }
    }

    /// Get a reference to the EventWriter for event persistence.
    fn writer(&self) -> EventWriter {
        EventWriter::new(self.pool.clone())
    }

    /// Bridge sync → async.
    /// Uses `block_in_place` inside a runtime (requires multi-thread flavor);
    /// creates a standalone runtime when called from plain sync code.
    fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(f)),
            Err(_) => {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("create tokio runtime");
                rt.block_on(f)
            }
        }
    }

    /// Rebuild the in-memory checkpoint from SQLite tables.
    fn rebuild_checkpoint(&self) -> anyhow::Result<Option<RunCheckpoint>> {
        self.block_on(async {
            let run_id = self.run_id;

            // Fetch checkpoint row
            let cp_row = sqlx::query(
                "SELECT status, current_phase, total_tokens, created_at, updated_at,
                        workflow_meta, started_agent_ids
                 FROM checkpoints WHERE run_id = ?",
            )
            .bind(run_id)
            .fetch_optional(&self.pool)
            .await?;

            let Some(row) = cp_row else {
                return Ok(None);
            };

            let status: String = row.try_get("status")?;
            let current_phase: u32 = row.try_get::<i64, _>("current_phase")? as u32;
            let total_tokens: u64 = row.try_get::<i64, _>("total_tokens")? as u64;
            let created_at: u64 = row.try_get::<i64, _>("created_at")? as u64;
            let updated_at: u64 = row.try_get::<i64, _>("updated_at")? as u64;
            let workflow_meta: Option<String> = row.try_get("workflow_meta")?;
            let started_agent_ids_json: String = row.try_get("started_agent_ids")?;

            // Fetch task from runs table
            let task: String =
                sqlx::query_scalar("SELECT task FROM runs WHERE run_id = ?")
                    .bind(run_id)
                    .fetch_one(&self.pool)
                    .await?;

            // Fetch phases
            let phase_rows = sqlx::query(
                "SELECT phase_id, label, planned, ok, failed, description, role
                 FROM phases WHERE run_id = ? ORDER BY phase_id",
            )
            .bind(run_id)
            .fetch_all(&self.pool)
            .await?;

            let completed_phases: Vec<PhaseSummary> = phase_rows
                .into_iter()
                .map(|r| PhaseSummary {
                    phase_id: r.try_get::<i64, _>("phase_id").unwrap_or(0) as u32,
                    label: r.try_get("label").unwrap_or_default(),
                    planned: r.try_get::<i64, _>("planned").unwrap_or(0) as usize,
                    ok: r.try_get::<i64, _>("ok").unwrap_or(0) as usize,
                    failed: r.try_get::<i64, _>("failed").unwrap_or(0) as usize,
                    description: r.try_get("description").unwrap_or(None),
                    role: r.try_get("role").unwrap_or(None),
                })
                .collect();

            // Fetch agent results
            let agent_rows = sqlx::query(
                "SELECT agent_id, phase_id, status, output, findings_json, input_tokens + output_tokens as tokens,
                        cache_key_hash, description, role, completed_at
                 FROM agents WHERE run_id = ? AND status != 'running'",
            )
            .bind(run_id)
            .fetch_all(&self.pool)
            .await?;

            let mut agent_results: HashMap<AgentId, AgentResultCache> = HashMap::new();
            for r in agent_rows {
                let agent_id_bytes: Vec<u8> = r.try_get("agent_id").unwrap_or_default();
                let agent_id = AgentId::from_slice(&agent_id_bytes).unwrap_or_default();
                let output_str: Option<String> = r.try_get("output").unwrap_or(None);
                let output: serde_json::Value = output_str
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or(serde_json::Value::Null);
                let findings_str: String = r.try_get("findings_json").unwrap_or_else(|_| "[]".into());
                let findings: Vec<Finding> =
                    serde_json::from_str(&findings_str).unwrap_or_default();

                agent_results.insert(
                    agent_id,
                    AgentResultCache {
                        agent_id,
                        phase_id: r.try_get::<i64, _>("phase_id").unwrap_or(0) as u32,
                        status: r.try_get("status").unwrap_or_else(|_| "unknown".into()),
                        output,
                        findings,
                        tokens: r.try_get::<i64, _>("tokens").unwrap_or(0) as u64,
                        completed_at: r.try_get::<i64, _>("completed_at").unwrap_or(0) as u64,
                        cache_key_hash: r.try_get("cache_key_hash").unwrap_or(None),
                        description: r.try_get("description").unwrap_or(None),
                        role: r.try_get("role").unwrap_or(None),
                    },
                );
            }

            // Fetch agent sessions
            let session_rows = sqlx::query(
                "SELECT agent_id, backend_id, protocol_session_id, session_id, status, updated_at, resumable
                 FROM agent_sessions WHERE run_id = ?",
            )
            .bind(run_id)
            .fetch_all(&self.pool)
            .await?;

            let mut agent_sessions: HashMap<AgentId, AgentSessionCheckpoint> = HashMap::new();
            for r in session_rows {
                let agent_id_bytes: Vec<u8> = r.try_get("agent_id").unwrap_or_default();
                let agent_id = AgentId::from_slice(&agent_id_bytes).unwrap_or_default();
                agent_sessions.insert(
                    agent_id,
                    AgentSessionCheckpoint {
                        agent_id,
                        backend_id: r.try_get("backend_id").unwrap_or(None),
                        protocol_session_id: r.try_get("protocol_session_id").unwrap_or(None),
                        session_id: r.try_get("session_id").unwrap_or_default(),
                        status: r.try_get("status").unwrap_or_default(),
                        updated_at: r.try_get::<i64, _>("updated_at").unwrap_or(0) as u64,
                        resumable: r.try_get::<i64, _>("resumable").unwrap_or(0) != 0,
                    },
                );
            }

            // Fetch findings
            let finding_rows = sqlx::query(
                "SELECT kind, severity, title, detail, file_path, line_start, line_end, evidence, data
                 FROM findings WHERE run_id = ?",
            )
            .bind(run_id)
            .fetch_all(&self.pool)
            .await?;

            let findings: Vec<Finding> = finding_rows
                .into_iter()
                .filter_map(|r| {
                    let data_str: Option<String> = r.try_get("data").ok()?;
                    serde_json::from_str(&data_str.unwrap_or_default()).ok()
                })
                .collect();

            let workflow_meta = workflow_meta
                .and_then(|s| serde_json::from_str(&s).ok());

            let started_agent_ids: Vec<AgentId> =
                serde_json::from_str(&started_agent_ids_json).unwrap_or_default();

            let checkpoint = RunCheckpoint {
                run_id,
                task,
                status: CheckpointStatus::parse_str(&status),
                current_phase,
                completed_phases,
                agent_results,
                agent_sessions,
                findings,
                total_tokens,
                created_at,
                updated_at,
                workflow_meta,
                started_agent_ids,
            };

            Ok(Some(checkpoint))
        })
    }

    /// Update checkpoint in-memory cache after an event.
    fn update_checkpoint_cache(&self, event: &AgentEvent) {
        let mut cache = self.checkpoint.write().unwrap();
        if let Some(ref mut cp) = *cache {
            cp.updated_at = luft_core::state::current_timestamp();

            match event {
                AgentEvent::PhaseStarted {
                    phase_id,
                    label,
                    planned,
                    description,
                    role,
                    ..
                } => {
                    cp.current_phase = cp.current_phase.max(*phase_id);
                    cp.completed_phases.push(PhaseSummary {
                        phase_id: *phase_id,
                        label: label.clone(),
                        planned: *planned,
                        ok: 0,
                        failed: 0,
                        description: description.clone(),
                        role: role.clone(),
                    });
                }
                AgentEvent::PhaseDone {
                    phase_id,
                    ok,
                    failed,
                    ..
                } => {
                    if let Some(phase) =
                        cp.completed_phases.iter_mut().find(|p| p.phase_id == *phase_id)
                    {
                        phase.ok = *ok;
                        phase.failed = *failed;
                    }
                }
                AgentEvent::AgentDone {
                    agent_id,
                    tokens,
                    status,
                    ..
                } => {
                    cp.total_tokens += tokens.total();
                    let existing = cp.agent_results.get(agent_id).cloned();
                    if let Some(mut existing) = existing {
                        existing.status = status.as_str().to_string();
                        existing.tokens = tokens.total();
                        existing.completed_at = luft_core::state::current_timestamp();
                        cp.agent_results.insert(*agent_id, existing);
                    }
                }
                AgentEvent::RunDone {
                    status, total_tokens, ..
                } => {
                    cp.total_tokens = total_tokens.total();
                    cp.status = match status {
                        luft_core::contract::event::RunStatus::Completed => {
                            CheckpointStatus::Completed
                        }
                        luft_core::contract::event::RunStatus::Failed => {
                            CheckpointStatus::Failed
                        }
                        luft_core::contract::event::RunStatus::Cancelled => {
                            CheckpointStatus::Cancelled
                        }
                        _ => CheckpointStatus::Completed,
                    };
                }
                _ => {}
            }
        }
    }

    /// Write event to events audit table.
    #[allow(dead_code)]
    fn write_event_audit(&self, event: &AgentEvent) -> anyhow::Result<()> {
        let payload = serde_json::to_string(event)?;
        let type_tag = event_type_tag(event);
        let run_id = self.run_id;
        self.block_on(async {
            sqlx::query("INSERT INTO events (run_id, type, payload) VALUES (?, ?, ?)")
                .bind(run_id)
                .bind(type_tag)
                .bind(&payload)
                .execute(&self.pool)
                .await?;
            Ok::<(), anyhow::Error>(())
        })
    }

    /// Update checkpoints table after an event.
    fn update_checkpoint_table(&self, event: &AgentEvent) -> anyhow::Result<()> {
        let run_id = self.run_id;
        let now = luft_core::state::current_timestamp();

        match event {
            AgentEvent::PhaseStarted { phase_id, .. } => {
                self.block_on(async {
                    sqlx::query(
                        "UPDATE checkpoints SET current_phase = MAX(current_phase, ?), updated_at = ? WHERE run_id = ?",
                    )
                    .bind(*phase_id as i64)
                    .bind(now as i64)
                    .bind(run_id)
                    .execute(&self.pool)
                    .await?;
                    Ok::<_, anyhow::Error>(())
                })?;
            }
            AgentEvent::AgentDone { tokens, .. } => {
                self.block_on(async {
                    sqlx::query(
                        "UPDATE checkpoints SET total_tokens = total_tokens + ?, updated_at = ? WHERE run_id = ?",
                    )
                    .bind(tokens.total() as i64)
                    .bind(now as i64)
                    .bind(run_id)
                    .execute(&self.pool)
                    .await?;
                    Ok::<_, anyhow::Error>(())
                })?;
            }
            AgentEvent::RunDone { status, total_tokens, .. } => {
                let status_str = match status {
                    luft_core::contract::event::RunStatus::Completed => "completed",
                    luft_core::contract::event::RunStatus::Failed => "failed",
                    luft_core::contract::event::RunStatus::Cancelled => "cancelled",
                    _ => "completed",
                };
                let total = total_tokens.total() as i64;
                self.block_on(async {
                    sqlx::query(
                        "UPDATE checkpoints SET status = ?, total_tokens = ?, updated_at = ? WHERE run_id = ?",
                    )
                    .bind(status_str)
                    .bind(total)
                    .bind(now as i64)
                    .bind(run_id)
                    .execute(&self.pool)
                    .await?;
                    Ok::<_, anyhow::Error>(())
                })?;
            }
            _ => {}
        }
        Ok::<(), anyhow::Error>(())
    }
}

impl CheckpointBackend for SqliteCheckpointBackend {
    fn init_run(&self, run_id: RunId, task: &str, run_dir: &str) -> anyhow::Result<()> {
        let now = luft_core::state::current_timestamp();
        self.block_on(async {
            sqlx::query(
                "INSERT OR IGNORE INTO runs (run_id, task, status, started_ts, run_dir)
                 VALUES (?, ?, 'running', ?, ?)",
            )
            .bind(run_id)
            .bind(task)
            .bind(format!("{}", now))
            .bind(run_dir)
            .execute(&self.pool)
            .await?;

            // Insert into checkpoints table
            sqlx::query(
                "INSERT OR REPLACE INTO checkpoints (run_id, status, current_phase, total_tokens, created_at, updated_at, started_agent_ids)
                 VALUES (?, 'running', 0, 0, ?, ?, '[]')",
            )
            .bind(run_id)
            .bind(now as i64)
            .bind(now as i64)
            .execute(&self.pool)
            .await?;

            Ok::<(), anyhow::Error>(())
        })?;

        // Initialize in-memory cache
        let cp = RunCheckpoint {
            run_id,
            task: task.to_string(),
            status: CheckpointStatus::Running,
            current_phase: 0,
            completed_phases: vec![],
            agent_results: HashMap::new(),
            agent_sessions: HashMap::new(),
            findings: vec![],
            total_tokens: 0,
            created_at: now,
            updated_at: now,
            workflow_meta: None,
            started_agent_ids: vec![],
        };
        *self.checkpoint.write().unwrap() = Some(cp);
        Ok::<(), anyhow::Error>(())
    }

    fn init_run_with_meta(
        &self,
        run_id: RunId,
        task: &str,
        run_dir: &str,
        workflow_meta: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.init_run(run_id, task, run_dir)?;
        let meta_str = serde_json::to_string(&workflow_meta)?;
        self.block_on(async {
            sqlx::query("UPDATE checkpoints SET workflow_meta = ? WHERE run_id = ?")
                .bind(&meta_str)
                .bind(run_id)
                .execute(&self.pool)
                .await?;
            Ok::<(), anyhow::Error>(())
        })?;

        // Update cache
        if let Some(ref mut cp) = *self.checkpoint.write().unwrap() {
            cp.workflow_meta = Some(workflow_meta);
        }
        Ok::<(), anyhow::Error>(())
    }

    fn open_run(&self, _run_id: RunId) -> anyhow::Result<Option<RunCheckpoint>> {
        let checkpoint = self.rebuild_checkpoint()?;
        if checkpoint.is_some() {
            *self.checkpoint.write().unwrap() = checkpoint.clone();
        }
        Ok(checkpoint)
    }

    fn append_event(&self, event: &AgentEvent) -> anyhow::Result<()> {
        // Safety net: ensure the event's run_id exists in runs table (FK constraint)
        let event_rid = event_run_id(event);
        if event_rid != uuid::Uuid::nil() && event_rid != self.run_id {
            let now = luft_core::state::current_timestamp();
            self.block_on(async {
                sqlx::query("INSERT OR IGNORE INTO runs (run_id, task, status, started_ts, run_dir) VALUES (?, '', 'running', ?, NULL)")
                    .bind(event_rid)
                    .bind(format!("{}", now))
                    .execute(&self.pool)
                    .await
            }).map_err(|e| anyhow::anyhow!("FK safety net failed: {e}"))?;
        }

        // 1. Update structured tables + events audit log (via EventWriter)
        self.block_on(async {
            self.writer().write_event(event).await
        }).map_err(|e| anyhow::anyhow!("EventWriter error: {e}"))?;

        // 2. Update checkpoints table
        self.update_checkpoint_table(event)?;

        // 3. Update in-memory cache
        self.update_checkpoint_cache(event);

        Ok::<(), anyhow::Error>(())
    }

    fn upsert_agent_result(&self, cache: &AgentResultCache) -> anyhow::Result<()> {
        let output_str = serde_json::to_string(&cache.output)?;
        let findings_str = serde_json::to_string(&cache.findings)?;
        let now = luft_core::state::current_timestamp();

        self.block_on(async {
            sqlx::query(
                "INSERT INTO agents (run_id, agent_id, phase_id, status, output, findings_json,
                     input_tokens, output_tokens, started_ts, done_ts, elapsed_ms,
                     cache_key_hash, description, role, completed_at, retry_count)
                 VALUES (?, ?, ?, ?, ?, ?, 0, 0, ?, ?, 0, ?, ?, ?, ?, 0)
                 ON CONFLICT(run_id, agent_id) DO UPDATE SET
                     status = excluded.status,
                     output = excluded.output,
                     findings_json = excluded.findings_json,
                     cache_key_hash = excluded.cache_key_hash,
                     description = excluded.description,
                     role = excluded.role,
                     completed_at = excluded.completed_at",
            )
            .bind(self.run_id)
            .bind(cache.agent_id)
            .bind(cache.phase_id as i64)
            .bind(&cache.status)
            .bind(&output_str)
            .bind(&findings_str)
            .bind(format!("{}", now))
            .bind(format!("{}", cache.completed_at))
            .bind(&cache.cache_key_hash)
            .bind(&cache.description)
            .bind(&cache.role)
            .bind(cache.completed_at as i64)
            .execute(&self.pool)
            .await?;
            Ok::<(), anyhow::Error>(())
        })?;

        // Update in-memory cache
        if let Some(ref mut cp) = *self.checkpoint.write().unwrap() {
            cp.agent_results.insert(cache.agent_id, cache.clone());
        }
        Ok::<(), anyhow::Error>(())
    }

    fn upsert_agent_session(&self, session: &AgentSessionCheckpoint) -> anyhow::Result<()> {
        self.block_on(async {
            sqlx::query(
                "INSERT INTO agent_sessions (run_id, agent_id, backend_id, protocol_session_id, session_id, status, updated_at, resumable)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(run_id, agent_id) DO UPDATE SET
                     backend_id = excluded.backend_id,
                     protocol_session_id = excluded.protocol_session_id,
                     session_id = excluded.session_id,
                     status = excluded.status,
                     updated_at = excluded.updated_at,
                     resumable = excluded.resumable",
            )
            .bind(self.run_id)
            .bind(session.agent_id)
            .bind(&session.backend_id)
            .bind(&session.protocol_session_id)
            .bind(&session.session_id)
            .bind(&session.status)
            .bind(session.updated_at as i64)
            .bind(session.resumable as i64)
            .execute(&self.pool)
            .await?;
            Ok::<(), anyhow::Error>(())
        })?;

        // Update in-memory cache
        if let Some(ref mut cp) = *self.checkpoint.write().unwrap() {
            cp.agent_sessions.insert(session.agent_id, session.clone());
        }
        Ok::<(), anyhow::Error>(())
    }

    fn get_checkpoint(&self) -> Option<RunCheckpoint> {
        self.checkpoint.read().unwrap().clone()
    }

    fn get_findings(&self) -> Vec<Finding> {
        self.checkpoint
            .read()
            .unwrap()
            .as_ref()
            .map(|cp| cp.findings.clone())
            .unwrap_or_default()
    }

    fn get_event_log(&self) -> anyhow::Result<Vec<AgentEvent>> {
        self.block_on(async {
            let rows = sqlx::query("SELECT payload FROM events WHERE run_id = ? ORDER BY seq")
                .bind(self.run_id)
                .fetch_all(&self.pool)
                .await?;

            let events: Vec<AgentEvent> = rows
                .into_iter()
                .filter_map(|r| {
                    let payload: String = r.try_get("payload").ok()?;
                    serde_json::from_str(&payload).ok()
                })
                .collect();
            Ok(events)
        })
    }

    fn can_resume(&self) -> bool {
        self.checkpoint
            .read()
            .unwrap()
            .as_ref()
            .map(|cp| {
                matches!(
                    cp.status,
                    CheckpointStatus::Running | CheckpointStatus::Failed | CheckpointStatus::Cancelled
                )
            })
            .unwrap_or(false)
    }

    fn reset_status_to_running(&self) -> anyhow::Result<()> {
        let now = luft_core::state::current_timestamp();
        self.block_on(async {
            sqlx::query(
                "UPDATE checkpoints SET status = 'running', updated_at = ? WHERE run_id = ?",
            )
            .bind(now as i64)
            .bind(self.run_id)
            .execute(&self.pool)
            .await?;
            Ok::<(), anyhow::Error>(())
        })?;

        if let Some(ref mut cp) = *self.checkpoint.write().unwrap() {
            cp.status = CheckpointStatus::Running;
            cp.updated_at = now;
        }
        Ok::<(), anyhow::Error>(())
    }

    fn cancel(&self) -> anyhow::Result<()> {
        let now = luft_core::state::current_timestamp();
        self.block_on(async {
            sqlx::query(
                "UPDATE checkpoints SET status = 'cancelled', updated_at = ? WHERE run_id = ? AND status = 'running'",
            )
            .bind(now as i64)
            .bind(self.run_id)
            .execute(&self.pool)
            .await?;
            Ok::<(), anyhow::Error>(())
        })?;

        if let Some(ref mut cp) = *self.checkpoint.write().unwrap() {
            cp.status = CheckpointStatus::Cancelled;
            cp.updated_at = now;
        }
        Ok::<(), anyhow::Error>(())
    }

    fn save_checkpoint(&self, checkpoint: &RunCheckpoint) -> anyhow::Result<()> {
        let meta_str = checkpoint
            .workflow_meta
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default());
        let started_ids_str = serde_json::to_string(&checkpoint.started_agent_ids)?;
        let now = luft_core::state::current_timestamp();
        let run_id = checkpoint.run_id;

        self.block_on(async {
            // 1. Upsert checkpoint row
            sqlx::query(
                "INSERT OR REPLACE INTO checkpoints
                    (run_id, status, current_phase, total_tokens, created_at, updated_at, workflow_meta, started_agent_ids)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(run_id)
            .bind(checkpoint.status.as_str())
            .bind(checkpoint.current_phase as i64)
            .bind(checkpoint.total_tokens as i64)
            .bind(checkpoint.created_at as i64)
            .bind(now as i64)
            .bind(&meta_str)
            .bind(&started_ids_str)
            .execute(&self.pool)
            .await?;

            // 2. Sync phases: delete + re-insert
            sqlx::query("DELETE FROM phases WHERE run_id = ?")
                .bind(run_id)
                .execute(&self.pool)
                .await?;

            for phase in &checkpoint.completed_phases {
                sqlx::query(
                    "INSERT INTO phases (run_id, phase_id, label, planned, ok, failed, description, role)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(run_id)
                .bind(phase.phase_id as i64)
                .bind(&phase.label)
                .bind(phase.planned as i64)
                .bind(phase.ok as i64)
                .bind(phase.failed as i64)
                .bind(&phase.description)
                .bind(&phase.role)
                .execute(&self.pool)
                .await?;
            }

            // 3. Sync agent results
            for (agent_id, cache) in &checkpoint.agent_results {
                let output_str = serde_json::to_string(&cache.output)?;
                let findings_str = serde_json::to_string(&cache.findings)?;
                sqlx::query(
                    "INSERT INTO agents (run_id, agent_id, phase_id, status, output, findings_json,
                         input_tokens, output_tokens, started_ts, done_ts, elapsed_ms,
                         cache_key_hash, description, role, completed_at, retry_count)
                     VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?, 0, ?, ?, ?, ?, 0)
                     ON CONFLICT(run_id, agent_id) DO UPDATE SET
                         status = excluded.status,
                         output = excluded.output,
                         findings_json = excluded.findings_json,
                         output_tokens = excluded.output_tokens,
                         cache_key_hash = excluded.cache_key_hash,
                         description = excluded.description,
                         role = excluded.role,
                         completed_at = excluded.completed_at",
                )
                .bind(run_id)
                .bind(agent_id)
                .bind(cache.phase_id as i64)
                .bind(&cache.status)
                .bind(&output_str)
                .bind(&findings_str)
                .bind(cache.tokens as i64)
                .bind(format!("{}", cache.completed_at))
                .bind(format!("{}", cache.completed_at))
                .bind(&cache.cache_key_hash)
                .bind(&cache.description)
                .bind(&cache.role)
                .bind(cache.completed_at as i64)
                .execute(&self.pool)
                .await?;
            }

            // 4. Sync agent sessions
            for (agent_id, session) in &checkpoint.agent_sessions {
                sqlx::query(
                    "INSERT INTO agent_sessions (run_id, agent_id, backend_id, protocol_session_id, session_id, status, updated_at, resumable)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                     ON CONFLICT(run_id, agent_id) DO UPDATE SET
                         backend_id = excluded.backend_id,
                         protocol_session_id = excluded.protocol_session_id,
                         session_id = excluded.session_id,
                         status = excluded.status,
                         updated_at = excluded.updated_at,
                         resumable = excluded.resumable",
                )
                .bind(run_id)
                .bind(agent_id)
                .bind(&session.backend_id)
                .bind(&session.protocol_session_id)
                .bind(&session.session_id)
                .bind(&session.status)
                .bind(session.updated_at as i64)
                .bind(session.resumable as i64)
                .execute(&self.pool)
                .await?;
            }

            // 5. Sync findings
            sqlx::query("DELETE FROM findings WHERE run_id = ?")
                .bind(run_id)
                .execute(&self.pool)
                .await?;
            for finding in &checkpoint.findings {
                let data = serde_json::to_string(finding)?;
                let (file_path, line) = finding.location.as_ref().map_or((String::new(), None), |l| {
                    (l.file.to_string_lossy().to_string(), l.line.map(|n| n as i64))
                });
                sqlx::query(
                    "INSERT INTO findings (run_id, kind, severity, title, detail, file_path, line_start, line_end, evidence, data)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(run_id)
                .bind(&finding.kind)
                .bind(format!("{:?}", finding.severity))
                .bind(&finding.title)
                .bind(&finding.detail)
                .bind(&file_path)
                .bind(line)
                .bind(line)
                .bind(serde_json::to_string(&finding.evidence).unwrap_or_default())
                .bind(&data)
                .execute(&self.pool)
                .await?;
            }

            Ok::<(), anyhow::Error>(())
        })?;

        *self.checkpoint.write().unwrap() = Some(checkpoint.clone());
        Ok::<(), anyhow::Error>(())
    }
}

/// Extract run_id from an AgentEvent via pattern matching.
fn event_run_id(event: &AgentEvent) -> RunId {
    match event {
        AgentEvent::RunStarted { run_id, .. }
        | AgentEvent::PhaseStarted { run_id, .. }
        | AgentEvent::AgentStarted { run_id, .. }
        | AgentEvent::AgentProgress { run_id, .. }
        | AgentEvent::AgentDone { run_id, .. }
        | AgentEvent::PhaseDone { run_id, .. }
        | AgentEvent::RunDone { run_id, .. }
        | AgentEvent::Log { run_id, .. }
        | AgentEvent::BudgetSet { run_id, .. }
        | AgentEvent::ReportEmitted { run_id, .. }
        | AgentEvent::ParallelStarted { run_id, .. }
        | AgentEvent::ParallelDone { run_id, .. }
        | AgentEvent::WorkflowStarted { run_id, .. }
        | AgentEvent::WorkflowDone { run_id, .. }
        | AgentEvent::ConvergeStarted { run_id, .. }
        | AgentEvent::ConvergeDone { run_id, .. }
        | AgentEvent::PipelineStarted { run_id, .. }
        | AgentEvent::PipelineStageStarted { run_id, .. }
        | AgentEvent::PipelineItemDone { run_id, .. }
        | AgentEvent::PipelineDone { run_id, .. } => *run_id,
        AgentEvent::SignalReceived { run_id, .. } => {
            run_id.unwrap_or_default()
        }
        _ => uuid::Uuid::nil(),
    }
}

/// Get a string type tag for an AgentEvent.
fn event_type_tag(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::RunStarted { .. } => "run_started",
        AgentEvent::PhaseStarted { .. } => "phase_started",
        AgentEvent::AgentStarted { .. } => "agent_started",
        AgentEvent::AgentProgress { .. } => "agent_progress",
        AgentEvent::AgentDone { .. } => "agent_done",
        AgentEvent::PhaseDone { .. } => "phase_done",
        AgentEvent::RunDone { .. } => "run_done",
        AgentEvent::Log { .. } => "log",
        AgentEvent::SignalReceived { .. } => "signal_received",        AgentEvent::BudgetSet { .. } => "budget_set",
        AgentEvent::ReportEmitted { .. } => "report_emitted",
        AgentEvent::ParallelStarted { .. } => "parallel_started",
        AgentEvent::ParallelDone { .. } => "parallel_done",
        AgentEvent::WorkflowStarted { .. } => "workflow_started",
        AgentEvent::WorkflowDone { .. } => "workflow_done",
        AgentEvent::ConvergeStarted { .. } => "converge_started",
        AgentEvent::ConvergeDone { .. } => "converge_done",
        AgentEvent::PipelineStarted { .. } => "pipeline_started",
        AgentEvent::PipelineStageStarted { .. } => "pipeline_stage_started",
        AgentEvent::PipelineItemDone { .. } => "pipeline_item_done",
        AgentEvent::PipelineDone { .. } => "pipeline_done",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use tempfile::tempdir;

    fn make_backend(run_id: RunId) -> SqliteCheckpointBackend {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        // Keep tempdir alive for the test by leaking it
        Box::leak(Box::new(dir));
        let pool = tokio::runtime::Runtime::new().unwrap().block_on(async {
            open_db(&db_path).await.unwrap()
        });
        SqliteCheckpointBackend::new(pool, run_id)
    }

    #[test]
    fn test_init_and_get_checkpoint() {
        let run_id = uuid::Uuid::now_v7();
        let backend = make_backend(run_id);
        backend.init_run(run_id, "Test task", "test_run").unwrap();

        let cp = backend.get_checkpoint().unwrap();
        assert_eq!(cp.run_id, run_id);
        assert_eq!(cp.task, "Test task");
        assert_eq!(cp.status, CheckpointStatus::Running);
    }

    #[test]
    fn test_cancel() {
        let run_id = uuid::Uuid::now_v7();
        let backend = make_backend(run_id);
        backend.init_run(run_id, "Cancel me", "cancel_run").unwrap();
        backend.cancel().unwrap();

        let cp = backend.get_checkpoint().unwrap();
        assert_eq!(cp.status, CheckpointStatus::Cancelled);
    }

    #[test]
    fn test_upsert_agent_result() {
        let run_id = uuid::Uuid::now_v7();
        let backend = make_backend(run_id);
        backend.init_run(run_id, "Agent test", "agent_run").unwrap();

        let agent_id = uuid::Uuid::now_v7();
        let cache = AgentResultCache {
            agent_id,
            phase_id: 1,
            status: "ok".to_string(),
            output: serde_json::json!({"result": "success"}),
            findings: vec![],
            tokens: 150,
            completed_at: luft_core::state::current_timestamp(),
            cache_key_hash: Some("abc123".to_string()),
            description: None,
            role: None,
        };
        backend.upsert_agent_result(&cache).unwrap();

        let cp = backend.get_checkpoint().unwrap();
        let cached = cp.agent_results.get(&agent_id).unwrap();
        assert_eq!(cached.status, "ok");
        assert_eq!(cached.tokens, 150);
    }

    #[test]
    fn test_open_run() {
        let run_id = uuid::Uuid::now_v7();
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let _dir_box = Box::leak(Box::new(dir));
        let pool = tokio::runtime::Runtime::new().unwrap().block_on(async {
            open_db(&db_path).await.unwrap()
        });

        let backend1 = SqliteCheckpointBackend::new(pool.clone(), run_id);
        backend1.init_run(run_id, "Persist test", "persist_run").unwrap();

        let agent_id = uuid::Uuid::now_v7();
        let cache = AgentResultCache {
            agent_id,
            phase_id: 0,
            status: "ok".to_string(),
            output: serde_json::json!({"survived": true}),
            findings: vec![],
            tokens: 2,
            completed_at: 0,
            cache_key_hash: Some("key1".to_string()),
            description: None,
            role: None,
        };
        backend1.upsert_agent_result(&cache).unwrap();
        drop(backend1);

        let backend2 = SqliteCheckpointBackend::new(pool, run_id);
        let cp = backend2.open_run(run_id).unwrap().unwrap();
        assert_eq!(cp.task, "Persist test");
        assert_eq!(cp.status, CheckpointStatus::Running);
        let cached = cp.agent_results.get(&agent_id).unwrap();
        assert_eq!(cached.output, serde_json::json!({"survived": true}));
    }
}
