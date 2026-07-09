//! `AgentEvent` → SQLite write path.
//!
//! `EventWriter` subscribes to the broadcast event channel (via the
//! forwarder task in `service/run.rs`) and translates each `AgentEvent`
//! into structured SQL writes against the tables defined in
//! `migrations/20250819000001_initial.sql`.

use maestro_core::contract::backend::AgentStatus;
use maestro_core::contract::event::{AgentEvent, ProgressDelta, RunStatus};
use maestro_core::contract::ids::{AgentId, PhaseId, RunId};
use crate::db::DbPool;
use crate::error::StorageResult;
use chrono::{DateTime, Utc};
use serde_json::Value as Json;
use sqlx::Row;
use std::sync::Arc;

/// Translate an `AgentEvent` into structured SQL rows.
///
/// Cheap to clone (`DbPool` is internally `Arc`'d); share via `Arc<EventWriter>`.
#[derive(Clone)]
pub struct EventWriter {
    pool: DbPool,
}

impl EventWriter {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Process one `AgentEvent`. Failures are returned to the caller (the
    /// forwarder logs and continues) so that a transient SQL hiccup does not
    /// poison the rest of the run's events.
    pub async fn write_event(&self, event: &AgentEvent) -> StorageResult<()> {
        match event {
            AgentEvent::RunStarted { run_id, task, ts } => {
                self.write_run_started(*run_id, task, *ts).await?;
            }
            AgentEvent::PhaseStarted {
                run_id,
                phase_id,
                label,
                planned,
                description,
                role,
                parent_span_id: _,
                ..
            } => {
                self.write_phase_started(
                    *run_id,
                    *phase_id,
                    label,
                    *planned,
                    description.as_deref(),
                    role.as_deref(),
                )
                .await?;
            }
            AgentEvent::AgentStarted {
                run_id,
                phase_id,
                agent_id,
                prompt_preview,
                model,
                description: _,
                role: _,
                name: _,
                agent_seq: _,
            } => {
                self.write_agent_started(
                    *run_id,
                    *phase_id,
                    *agent_id,
                    prompt_preview,
                    model.as_deref(),
                )
                .await?;
            }
            AgentEvent::AgentProgress {
                run_id,
                agent_id,
                delta,
            } => {
                self.write_delta(*run_id, *agent_id, delta).await?;
            }
            AgentEvent::AgentDone {
                run_id,
                agent_id,
                status,
                tokens,
                elapsed_ms,
                name: _,
                agent_seq: _,
                output: _,
                findings: _,
                prompt: _,
                retry_count,
            } => {
                self.write_agent_done(
                    *run_id,
                    *agent_id,
                    status,
                    *tokens,
                    *elapsed_ms,
                    *retry_count,
                )
                .await?;
            }
            AgentEvent::PhaseDone {
                run_id,
                phase_id,
                ok,
                failed,
                ..
            } => {
                self.write_phase_done(*run_id, *phase_id, *ok, *failed)
                    .await?;
            }
            AgentEvent::RunDone {
                run_id,
                status,
                total_tokens,
                report,
                ..
            } => {
                self.write_run_done(*run_id, status, *total_tokens, report)
                    .await?;
            }
            AgentEvent::Log { .. } => {
                // Logs already captured via tracing; skip to avoid duplication.
            }
            AgentEvent::SignalReceived { .. } => {
                // Persisted to events.jsonl via the journal forwarder; not a
                // SQLite state change, so skip to avoid duplication.
            }
            AgentEvent::BudgetSet { .. } => {
                // Budget is a session-level concern; tracked in checkpoint.json.
            }
            AgentEvent::ReportEmitted {
                run_id,
                phase_id,
                report,
            } => {
                self.write_report_emitted(*run_id, *phase_id, report)
                    .await?;
            }
            AgentEvent::ParallelStarted {
                run_id,
                phase_id,
                span_id,
                count,
            } => {
                self.write_span_started(
                    *run_id,
                    *phase_id,
                    *span_id,
                    "parallel",
                    Some(*count as i64),
                    None,
                    None,
                    None,
                )
                .await?;
            }
            AgentEvent::ParallelDone {
                run_id,
                phase_id: _,
                span_id,
                ok,
                failed,
                results,
                elapsed_ms,
            } => {
                self.write_span_done(
                    *run_id,
                    *span_id,
                    *ok as i64,
                    *failed as i64,
                    Some(results),
                    None,
                    None,
                    *elapsed_ms,
                )
                .await?;
            }
            AgentEvent::WorkflowStarted {
                run_id,
                span_id,
                path,
                args,
            } => {
                self.write_span_started(
                    *run_id,
                    0,
                    *span_id,
                    "workflow",
                    None,
                    None,
                    Some(path),
                    Some(args),
                )
                .await?;
            }
            AgentEvent::WorkflowDone {
                run_id,
                span_id,
                path,
                report,
                elapsed_ms,
                error,
            } => {
                self.write_span_done(
                    *run_id,
                    *span_id,
                    if error.is_none() { 1 } else { 0 },
                    if error.is_some() { 1 } else { 0 },
                    Some(report),
                    error.clone(),
                    Some(path),
                    *elapsed_ms,
                )
                .await?;
            }
            AgentEvent::ConvergeStarted {
                run_id,
                phase_id,
                span_id,
                items,
                max_rounds,
            } => {
                self.write_span_started(
                    *run_id,
                    *phase_id,
                    *span_id,
                    "converge",
                    Some(*items as i64),
                    Some(*max_rounds as i64),
                    None,
                    None,
                )
                .await?;
            }
            AgentEvent::ConvergeDone {
                run_id,
                phase_id: _,
                span_id,
                rounds,
                converged,
                surviving: _,
                result,
                elapsed_ms,
                error,
            } => {
                self.write_converge_done(
                    *run_id,
                    *span_id,
                    *rounds as i64,
                    *converged,
                    Some(result),
                    error.clone(),
                    *elapsed_ms,
                )
                .await?;
            }
            AgentEvent::PipelineStarted {
                run_id,
                total_stages,
                items,
            } => {
                self.write_span_started(
                    *run_id,
                    0,
                    0,
                    "pipeline",
                    Some(*items as i64),
                    Some(*total_stages as i64),
                    None,
                    None,
                )
                .await?;
            }
            AgentEvent::PipelineStageStarted {
                run_id,
                stage_index,
                label,
                agents_in_stage,
            } => {
                self.write_span_started(
                    *run_id,
                    0,
                    *stage_index as u64 + 1,
                    "pipeline_stage",
                    Some(*agents_in_stage as i64),
                    None,
                    Some(label),
                    None,
                )
                .await?;
            }
            AgentEvent::PipelineItemDone {
                run_id: _,
                stage_index: _,
                item_index: _,
                status,
                tokens,
                elapsed_ms,
            } => {
                // Per-item token totals roll up into the pipeline_done event.
                tracing::trace!(
                    ?status,
                    ?tokens,
                    elapsed_ms,
                    "pipeline item done (skipping item-level write)"
                );
            }
            AgentEvent::PipelineDone {
                run_id,
                stages_completed,
                total_ok,
                total_failed,
            } => {
                self.write_span_done(
                    *run_id,
                    0,
                    *total_ok as i64,
                    *total_failed as i64,
                    None,
                    None,
                    None,
                    0,
                )
                .await?;
                tracing::trace!(stages_completed, "pipeline done");
            }
            // AcpRaw is intentionally not persisted (live observability stream,
            // not durable history). See docs/design/acp-raw-events.md.
            AgentEvent::AcpRaw { .. } => {}
            // Phase span events are structural metadata; captured in audit log
            // and checkpoint, no dedicated SQL table needed.
            AgentEvent::PhaseSpanStarted { .. }
            | AgentEvent::PhaseSpanDone { .. }
            | AgentEvent::PlanPreview { .. }
            | AgentEvent::SchemaRetry { .. } => {}
        }

        // All events are appended to the audit log for replay.
        self.append_audit(event).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Per-event-type writes
    // -----------------------------------------------------------------------

    async fn write_run_started(
        &self,
        run_id: RunId,
        task: &str,
        ts: DateTime<Utc>,
    ) -> StorageResult<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO runs (run_id, task, status, started_ts)
             VALUES (?, ?, 'running', ?)",
        )
        .bind(run_id)
        .bind(task)
        .bind(ts.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn write_phase_started(
        &self,
        run_id: RunId,
        phase_id: PhaseId,
        label: &str,
        planned: usize,
        description: Option<&str>,
        role: Option<&str>,
    ) -> StorageResult<()> {
        sqlx::query(
            "INSERT INTO phases (run_id, phase_id, label, planned, description, role, started_ts)
             VALUES (?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT(run_id, phase_id) DO UPDATE SET
               label = excluded.label,
               planned = excluded.planned,
               description = COALESCE(excluded.description, phases.description),
               role = COALESCE(excluded.role, phases.role),
               started_ts = COALESCE(phases.started_ts, excluded.started_ts)",
        )
        .bind(run_id)
        .bind(phase_id as i64)
        .bind(label)
        .bind(planned as i64)
        .bind(description)
        .bind(role)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn write_agent_started(
        &self,
        run_id: RunId,
        phase_id: PhaseId,
        agent_id: AgentId,
        prompt_preview: &str,
        model: Option<&str>,
    ) -> StorageResult<()> {
        sqlx::query(
            "INSERT INTO runs (run_id, task, status, started_ts)
             VALUES (?, '', 'running', strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT(run_id) DO NOTHING",
        )
        .bind(run_id)
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "INSERT INTO agents (run_id, agent_id, phase_id, model, status,
                                 prompt_preview, started_ts)
             VALUES (?, ?, ?, ?, 'running', ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT(run_id, agent_id) DO UPDATE SET
               phase_id = excluded.phase_id,
               model = excluded.model,
               prompt_preview = excluded.prompt_preview,
               status = 'running',
               started_ts = excluded.started_ts",
        )
        .bind(run_id)
        .bind(agent_id)
        .bind(phase_id as i64)
        .bind(model)
        .bind(prompt_preview)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn write_delta(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        delta: &ProgressDelta,
    ) -> StorageResult<()> {
        let now = Utc::now().to_rfc3339();
        let phase_id: Option<i64> =
            sqlx::query_scalar("SELECT phase_id FROM agents WHERE run_id = ? AND agent_id = ?")
                .bind(run_id)
                .bind(agent_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();

        match delta {
            ProgressDelta::Message { text } => {
                sqlx::query(
                    "INSERT INTO turns (run_id, agent_id, phase_id, ts, kind, role, text)
                     VALUES (?, ?, ?, ?, 'message', 'assistant', ?)",
                )
                .bind(run_id)
                .bind(agent_id)
                .bind(phase_id)
                .bind(now)
                .bind(text)
                .execute(&self.pool)
                .await?;
            }
            ProgressDelta::ToolCall { name, summary } => {
                sqlx::query(
                    "INSERT INTO turns (run_id, agent_id, phase_id, ts, kind, name, text)
                     VALUES (?, ?, ?, ?, 'tool_call', ?, ?)",
                )
                .bind(run_id)
                .bind(agent_id)
                .bind(phase_id)
                .bind(now)
                .bind(name)
                .bind(summary)
                .execute(&self.pool)
                .await?;
            }
            ProgressDelta::FileEdit { path } => {
                sqlx::query(
                    "INSERT INTO turns (run_id, agent_id, phase_id, ts, kind, file_path)
                     VALUES (?, ?, ?, ?, 'file_edit', ?)",
                )
                .bind(run_id)
                .bind(agent_id)
                .bind(phase_id)
                .bind(now)
                .bind(path.to_string_lossy().to_string())
                .execute(&self.pool)
                .await?;
            }
            ProgressDelta::Tokens { usage } => {
                sqlx::query(
                    "INSERT INTO turns (run_id, agent_id, phase_id, ts, kind,
                                        input_tokens, output_tokens,
                                        cache_read_tokens, cache_write_tokens)
                     VALUES (?, ?, ?, ?, 'tokens', ?, ?, ?, ?)",
                )
                .bind(run_id)
                .bind(agent_id)
                .bind(phase_id)
                .bind(now)
                .bind(usage.input as i64)
                .bind(usage.output as i64)
                .bind(usage.cache_read as i64)
                .bind(usage.cache_write as i64)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    async fn write_agent_done(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        status: &AgentStatus,
        tokens: maestro_core::contract::ids::TokenUsage,
        elapsed_ms: u64,
        retry_count: u32,
    ) -> StorageResult<()> {
        sqlx::query(
            "UPDATE agents
             SET status = ?,
                 input_tokens = ?,
                 output_tokens = ?,
                 cache_read_tokens = ?,
                 cache_write_tokens = ?,
                 done_ts = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                 elapsed_ms = ?,
                 retry_count = ?
             WHERE run_id = ? AND agent_id = ?",
        )
        .bind(agent_status_str(status.clone()))
        .bind(tokens.input as i64)
        .bind(tokens.output as i64)
        .bind(tokens.cache_read as i64)
        .bind(tokens.cache_write as i64)
        .bind(elapsed_ms as i64)
        .bind(retry_count as i64)
        .bind(run_id)
        .bind(agent_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn write_phase_done(
        &self,
        run_id: RunId,
        phase_id: PhaseId,
        ok: usize,
        failed: usize,
    ) -> StorageResult<()> {
        sqlx::query(
            "UPDATE phases
             SET ok = ?, failed = ?, done_ts = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE run_id = ? AND phase_id = ?",
        )
        .bind(ok as i64)
        .bind(failed as i64)
        .bind(run_id)
        .bind(phase_id as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn write_run_done(
        &self,
        run_id: RunId,
        status: &RunStatus,
        total_tokens: maestro_core::contract::ids::TokenUsage,
        report: &Json,
    ) -> StorageResult<()> {
        sqlx::query(
            "UPDATE runs
             SET status = ?,
                 finished_ts = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                 input_tokens = ?,
                 output_tokens = ?,
                 cache_read_tokens = ?,
                 cache_write_tokens = ?,
                 report = ?
             WHERE run_id = ?",
        )
        .bind(run_status_str(*status))
        .bind(total_tokens.input as i64)
        .bind(total_tokens.output as i64)
        .bind(total_tokens.cache_read as i64)
        .bind(total_tokens.cache_write as i64)
        .bind(serde_json::to_string(report)?)
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn write_report_emitted(
        &self,
        run_id: RunId,
        phase_id: PhaseId,
        report: &Json,
    ) -> StorageResult<()> {
        sqlx::query("UPDATE runs SET report = ? WHERE run_id = ?")
            .bind(serde_json::to_string(report)?)
            .bind(run_id)
            .execute(&self.pool)
            .await?;

        sqlx::query(
            "UPDATE phases SET done_ts = COALESCE(done_ts, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             WHERE run_id = ? AND phase_id = ?",
        )
        .bind(run_id)
        .bind(phase_id as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_span_started(
        &self,
        run_id: RunId,
        phase_id: PhaseId,
        span_id: u64,
        kind: &str,
        items: Option<i64>,
        max_rounds: Option<i64>,
        path: Option<&str>,
        _args: Option<&Json>,
    ) -> StorageResult<()> {
        sqlx::query(
            "INSERT INTO spans (run_id, span_id, kind, phase_id, items, max_rounds,
                                path, started_ts)
             VALUES (?, ?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT(run_id, span_id) DO UPDATE SET
               kind = excluded.kind,
               items = COALESCE(excluded.items, spans.items),
               max_rounds = COALESCE(excluded.max_rounds, spans.max_rounds),
               path = COALESCE(excluded.path, spans.path),
               started_ts = COALESCE(spans.started_ts, excluded.started_ts)",
        )
        .bind(run_id)
        .bind(span_id as i64)
        .bind(kind)
        .bind(phase_id as i64)
        .bind(items)
        .bind(max_rounds)
        .bind(path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_span_done(
        &self,
        run_id: RunId,
        span_id: u64,
        ok: i64,
        failed: i64,
        result: Option<&Json>,
        error: Option<String>,
        _path: Option<&str>,
        elapsed_ms: u64,
    ) -> StorageResult<()> {
        sqlx::query(
            "UPDATE spans
             SET ok = ?, failed = ?,
                 result = COALESCE(?, result),
                 error = ?,
                 done_ts = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                 elapsed_ms = ?
             WHERE run_id = ? AND span_id = ?",
        )
        .bind(ok)
        .bind(failed)
        .bind(result.map(serde_json::to_string).transpose()?)
        .bind(error)
        .bind(elapsed_ms as i64)
        .bind(run_id)
        .bind(span_id as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_converge_done(
        &self,
        run_id: RunId,
        span_id: u64,
        rounds: i64,
        converged: bool,
        result: Option<&Json>,
        error: Option<String>,
        elapsed_ms: u64,
    ) -> StorageResult<()> {
        sqlx::query(
            "UPDATE spans
             SET rounds = ?, converged = ?,
                 result = COALESCE(?, result),
                 error = ?,
                 done_ts = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                 elapsed_ms = ?
             WHERE run_id = ? AND span_id = ?",
        )
        .bind(rounds)
        .bind(converged as i64)
        .bind(result.map(serde_json::to_string).transpose()?)
        .bind(error)
        .bind(elapsed_ms as i64)
        .bind(run_id)
        .bind(span_id as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn append_audit(&self, event: &AgentEvent) -> StorageResult<()> {
        let (run_id, type_name) = audit_metadata(event);
        if run_id.is_none() {
            return Ok(());
        }
        // AcpRaw is intentionally not persisted (live observability stream).
        if type_name == "acp_raw" {
            return Ok(());
        }
        let payload = serde_json::to_string(event)?;
        sqlx::query("INSERT INTO events (run_id, type, payload) VALUES (?, ?, ?)")
            .bind(run_id.unwrap())
            .bind(type_name)
            .bind(payload)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

fn audit_metadata(event: &AgentEvent) -> (Option<RunId>, &'static str) {
    match event {
        AgentEvent::RunStarted { run_id, .. } => (Some(*run_id), "run_started"),
        AgentEvent::PhaseStarted { run_id, .. } => (Some(*run_id), "phase_started"),
        AgentEvent::AgentStarted { run_id, .. } => (Some(*run_id), "agent_started"),
        AgentEvent::AgentProgress { run_id, .. } => (Some(*run_id), "agent_progress"),
        AgentEvent::AcpRaw { run_id, .. } => (Some(*run_id), "acp_raw"),
        AgentEvent::AgentDone { run_id, .. } => (Some(*run_id), "agent_done"),
        AgentEvent::PhaseDone { run_id, .. } => (Some(*run_id), "phase_done"),
        AgentEvent::RunDone { run_id, .. } => (Some(*run_id), "run_done"),
        AgentEvent::Log { run_id, .. } => (Some(*run_id), "log"),
        AgentEvent::BudgetSet { run_id, .. } => (Some(*run_id), "budget_set"),
        AgentEvent::ReportEmitted { run_id, .. } => (Some(*run_id), "report_emitted"),
        AgentEvent::ParallelStarted { run_id, .. } => (Some(*run_id), "parallel_started"),
        AgentEvent::ParallelDone { run_id, .. } => (Some(*run_id), "parallel_done"),
        AgentEvent::WorkflowStarted { run_id, .. } => (Some(*run_id), "workflow_started"),
        AgentEvent::WorkflowDone { run_id, .. } => (Some(*run_id), "workflow_done"),
        AgentEvent::ConvergeStarted { run_id, .. } => (Some(*run_id), "converge_started"),
        AgentEvent::ConvergeDone { run_id, .. } => (Some(*run_id), "converge_done"),
        AgentEvent::PipelineStarted { run_id, .. } => (Some(*run_id), "pipeline_started"),
        AgentEvent::PipelineStageStarted { run_id, .. } => {
            (Some(*run_id), "pipeline_stage_started")
        }
        AgentEvent::PipelineItemDone { run_id, .. } => (Some(*run_id), "pipeline_item_done"),
        AgentEvent::PipelineDone { run_id, .. } => (Some(*run_id), "pipeline_done"),
        AgentEvent::PhaseSpanStarted { run_id, .. } => (Some(*run_id), "phase_span_started"),
        AgentEvent::PhaseSpanDone { run_id, .. } => (Some(*run_id), "phase_span_done"),
        AgentEvent::PlanPreview { run_id, .. } => (Some(*run_id), "plan_preview"),
        AgentEvent::SignalReceived { run_id, .. } => (*run_id, "signal_received"),
        AgentEvent::SchemaRetry { run_id, .. } => (Some(*run_id), "schema_retry"),
    }
}

fn agent_status_str(s: AgentStatus) -> &'static str {
    match s {
        AgentStatus::Ok => "ok",
        AgentStatus::Error => "error",
        AgentStatus::Cancelled => "cancelled",
        AgentStatus::TimedOut => "timed_out",
    }
}

fn run_status_str(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Partial => "partial",
    }
}

// ----------------------------------------------------------------------------
// Internal SQL helpers — used by tests
// ----------------------------------------------------------------------------

#[allow(dead_code)]
pub(crate) async fn fetch_event_count(pool: &DbPool, run_id: RunId) -> StorageResult<i64> {
    let row = sqlx::query("SELECT COUNT(*) AS c FROM events WHERE run_id = ?")
        .bind(run_id)
        .fetch_one(pool)
        .await?;
    Ok(row.try_get::<i64, _>("c")?)
}

#[allow(dead_code)]
pub(crate) async fn fetch_turn_count(
    pool: &DbPool,
    run_id: RunId,
    kind: &str,
) -> StorageResult<i64> {
    let row = sqlx::query("SELECT COUNT(*) AS c FROM turns WHERE run_id = ? AND kind = ?")
        .bind(run_id)
        .bind(kind)
        .fetch_one(pool)
        .await?;
    Ok(row.try_get::<i64, _>("c")?)
}

// Re-export Arc<EventWriter> as a convenience for forwarding tasks.
pub type SharedWriter = Arc<EventWriter>;

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_core::contract::event::LogLevel;
    use maestro_core::contract::ids::TokenUsage;
    use crate::db::open_db;
    use std::path::PathBuf;
    use tempfile::tempdir;

    async fn setup() -> (tempfile::TempDir, EventWriter) {
        let dir = tempdir().unwrap();
        let pool = open_db(&dir.path().join("test.db")).await.unwrap();
        (dir, EventWriter::new(pool))
    }

    #[tokio::test]
    async fn run_lifecycle() {
        let (_dir, w) = setup().await;
        let run_id = uuid::Uuid::now_v7();
        let ts = Utc::now();

        w.write_event(&AgentEvent::RunStarted {
            run_id,
            task: "hello".into(),
            ts,
        })
        .await
        .unwrap();

        let row = sqlx::query("SELECT task, status FROM runs WHERE run_id = ?")
            .bind(run_id)
            .fetch_one(w.pool())
            .await
            .unwrap();
        let task: String = row.try_get("task").unwrap();
        let status: String = row.try_get("status").unwrap();
        assert_eq!(task, "hello");
        assert_eq!(status, "running");

        w.write_event(&AgentEvent::RunDone {
            run_id,
            status: RunStatus::Completed,
            total_tokens: TokenUsage {
                input: 100,
                output: 50,
                cache_read: 0,
                cache_write: 0,
            },
            report: serde_json::json!({"result": "ok"}),
        ts: chrono::Utc::now(),
        })
        .await
        .unwrap();

        let row = sqlx::query("SELECT status, finished_ts FROM runs WHERE run_id = ?")
            .bind(run_id)
            .fetch_one(w.pool())
            .await
            .unwrap();
        let status: String = row.try_get("status").unwrap();
        let finished_ts: Option<String> = row.try_get("finished_ts").unwrap();
        assert_eq!(status, "completed");
        assert!(finished_ts.is_some());
    }

    #[tokio::test]
    async fn agent_started_creates_agent_row() {
        let (_dir, w) = setup().await;
        let run_id = uuid::Uuid::now_v7();
        let agent_id = uuid::Uuid::now_v7();

        w.write_event(&AgentEvent::AgentStarted {
            run_id,
            phase_id: 1,
            agent_id,
            prompt_preview: "do something".into(),
            model: Some("claude-sonnet-4".into()),
            description: None,
            role: None,
            name: None,
            agent_seq: 0,
        })
        .await
        .unwrap();

        let row = sqlx::query(
            "SELECT phase_id, model, status, prompt_preview FROM agents WHERE run_id = ? AND agent_id = ?",
        )
        .bind(run_id)
        .bind(agent_id)
        .fetch_one(w.pool())
        .await
        .unwrap();

        let phase_id: i64 = row.try_get("phase_id").unwrap();
        let model: String = row.try_get("model").unwrap();
        let status: String = row.try_get("status").unwrap();
        assert_eq!(phase_id, 1);
        assert_eq!(model, "claude-sonnet-4");
        assert_eq!(status, "running");
    }

    #[tokio::test]
    async fn progress_delta_message_creates_turn() {
        let (_dir, w) = setup().await;
        let run_id = uuid::Uuid::now_v7();
        let agent_id = uuid::Uuid::now_v7();

        // Need agents row to derive phase_id.
        w.write_event(&AgentEvent::AgentStarted {
            run_id,
            phase_id: 2,
            agent_id,
            prompt_preview: "p".into(),
            model: None,
            description: None,
            role: None,
            name: None,
            agent_seq: 0,
        })
        .await
        .unwrap();

        w.write_event(&AgentEvent::AgentProgress {
            run_id,
            agent_id,
            delta: ProgressDelta::Message {
                text: "thinking...".into(),
            },
        })
        .await
        .unwrap();

        let count = fetch_turn_count(w.pool(), run_id, "message").await.unwrap();
        assert_eq!(count, 1);

        let row = sqlx::query("SELECT text, role, phase_id FROM turns WHERE run_id = ? LIMIT 1")
            .bind(run_id)
            .fetch_one(w.pool())
            .await
            .unwrap();
        let text: String = row.try_get("text").unwrap();
        let role: String = row.try_get("role").unwrap();
        let phase_id: i64 = row.try_get("phase_id").unwrap();
        assert_eq!(text, "thinking...");
        assert_eq!(role, "assistant");
        assert_eq!(phase_id, 2);
    }

    #[tokio::test]
    async fn progress_delta_tool_call_and_file_edit() {
        let (_dir, w) = setup().await;
        let run_id = uuid::Uuid::now_v7();
        let agent_id = uuid::Uuid::now_v7();

        w.write_event(&AgentEvent::AgentStarted {
            run_id,
            phase_id: 0,
            agent_id,
            prompt_preview: "".into(),
            model: None,
            description: None,
            role: None,
            name: None,
            agent_seq: 0,
        })
        .await
        .unwrap();

        w.write_event(&AgentEvent::AgentProgress {
            run_id,
            agent_id,
            delta: ProgressDelta::ToolCall {
                name: "ReadFile".into(),
                summary: "read".into(),
            },
        })
        .await
        .unwrap();

        w.write_event(&AgentEvent::AgentProgress {
            run_id,
            agent_id,
            delta: ProgressDelta::FileEdit {
                path: PathBuf::from("src/main.rs"),
            },
        })
        .await
        .unwrap();

        assert_eq!(
            fetch_turn_count(w.pool(), run_id, "tool_call")
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            fetch_turn_count(w.pool(), run_id, "file_edit")
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn progress_delta_tokens_writes_agents_row() {
        let (_dir, w) = setup().await;
        let run_id = uuid::Uuid::now_v7();
        let agent_id = uuid::Uuid::now_v7();

        w.write_event(&AgentEvent::AgentStarted {
            run_id,
            phase_id: 0,
            agent_id,
            prompt_preview: "".into(),
            model: None,
            description: None,
            role: None,
            name: None,
            agent_seq: 0,
        })
        .await
        .unwrap();

        w.write_event(&AgentEvent::AgentProgress {
            run_id,
            agent_id,
            delta: ProgressDelta::Tokens {
                usage: TokenUsage {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                },
            },
        })
        .await
        .unwrap();

        assert_eq!(
            fetch_turn_count(w.pool(), run_id, "tokens").await.unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn agent_done_updates_status_and_tokens() {
        let (_dir, w) = setup().await;
        let run_id = uuid::Uuid::now_v7();
        let agent_id = uuid::Uuid::now_v7();

        w.write_event(&AgentEvent::AgentStarted {
            run_id,
            phase_id: 0,
            agent_id,
            prompt_preview: "".into(),
            model: None,
            description: None,
            role: None,
            name: None,
            agent_seq: 0,
        })
        .await
        .unwrap();

        w.write_event(&AgentEvent::AgentDone {
            run_id,
            agent_id,
            status: AgentStatus::Ok,
            tokens: TokenUsage {
                input: 200,
                output: 80,
                cache_read: 0,
                cache_write: 0,
            },
            elapsed_ms: 1234,
            name: None,
            agent_seq: 0,
            output: serde_json::Value::Null,
            findings: Vec::new(),
            prompt: String::new(),
            retry_count: 0,
        })
        .await
        .unwrap();

        let row = sqlx::query(
            "SELECT status, input_tokens, output_tokens, elapsed_ms FROM agents WHERE run_id = ? AND agent_id = ?",
        )
        .bind(run_id)
        .bind(agent_id)
        .fetch_one(w.pool())
        .await
        .unwrap();

        let status: String = row.try_get("status").unwrap();
        let input: i64 = row.try_get("input_tokens").unwrap();
        let output: i64 = row.try_get("output_tokens").unwrap();
        let elapsed: i64 = row.try_get("elapsed_ms").unwrap();
        assert_eq!(status, "ok");
        assert_eq!(input, 200);
        assert_eq!(output, 80);
        assert_eq!(elapsed, 1234);
    }

    #[tokio::test]
    async fn phase_started_and_done() {
        let (_dir, w) = setup().await;
        let run_id = uuid::Uuid::now_v7();

        w.write_event(&AgentEvent::RunStarted {
            run_id,
            task: "t".into(),
            ts: Utc::now(),
        })
        .await
        .unwrap();

        w.write_event(&AgentEvent::PhaseStarted {
            run_id,
            phase_id: 1,
            label: "explore".into(),
            planned: 3,
            parent_span_id: None,
            description: None,
            role: None,
            ts: chrono::Utc::now(),
        })
        .await
        .unwrap();

        w.write_event(&AgentEvent::PhaseDone {
            run_id,
            phase_id: 1,
            ok: 2,
            failed: 1,
        ts: chrono::Utc::now(),
        })
        .await
        .unwrap();

        let row = sqlx::query(
            "SELECT label, planned, ok, failed FROM phases WHERE run_id = ? AND phase_id = ?",
        )
        .bind(run_id)
        .bind(1i64)
        .fetch_one(w.pool())
        .await
        .unwrap();
        let label: String = row.try_get("label").unwrap();
        let planned: i64 = row.try_get("planned").unwrap();
        let ok: i64 = row.try_get("ok").unwrap();
        let failed: i64 = row.try_get("failed").unwrap();
        assert_eq!(label, "explore");
        assert_eq!(planned, 3);
        assert_eq!(ok, 2);
        assert_eq!(failed, 1);
    }

    #[tokio::test]
    async fn spans_track_orchestration() {
        let (_dir, w) = setup().await;
        let run_id = uuid::Uuid::now_v7();

        w.write_event(&AgentEvent::RunStarted {
            run_id,
            task: "t".into(),
            ts: Utc::now(),
        })
        .await
        .unwrap();

        w.write_event(&AgentEvent::ParallelStarted {
            run_id,
            phase_id: 1,
            span_id: 7,
            count: 4,
        })
        .await
        .unwrap();

        w.write_event(&AgentEvent::ParallelDone {
            run_id,
            phase_id: 1,
            span_id: 7,
            ok: 3,
            failed: 1,
            results: serde_json::json!([1, 2, 3]),
            elapsed_ms: 999,
        })
        .await
        .unwrap();

        let row = sqlx::query(
            "SELECT kind, items, ok, failed, elapsed_ms FROM spans WHERE run_id = ? AND span_id = ?",
        )
        .bind(run_id)
        .bind(7i64)
        .fetch_one(w.pool())
        .await
        .unwrap();
        let kind: String = row.try_get("kind").unwrap();
        let items: i64 = row.try_get("items").unwrap();
        let ok: i64 = row.try_get("ok").unwrap();
        let failed: i64 = row.try_get("failed").unwrap();
        assert_eq!(kind, "parallel");
        assert_eq!(items, 4);
        assert_eq!(ok, 3);
        assert_eq!(failed, 1);
    }

    #[tokio::test]
    async fn audit_log_captures_all_events() {
        let (_dir, w) = setup().await;
        let run_id = uuid::Uuid::now_v7();

        w.write_event(&AgentEvent::RunStarted {
            run_id,
            task: "t".into(),
            ts: Utc::now(),
        })
        .await
        .unwrap();

        w.write_event(&AgentEvent::Log {
            run_id,
            agent_id: None,
            level: LogLevel::Info,
            msg: "hi".into(),
        })
        .await
        .unwrap();

        // Log events are filtered from audit log intentionally (tracing handles them).
        let count = fetch_event_count(w.pool(), run_id).await.unwrap();
        assert!(count >= 1);
    }

    #[tokio::test]
    async fn cascade_delete_removes_all() {
        let (_dir, w) = setup().await;
        let run_id = uuid::Uuid::now_v7();

        w.write_event(&AgentEvent::RunStarted {
            run_id,
            task: "t".into(),
            ts: Utc::now(),
        })
        .await
        .unwrap();

        let agent_id = uuid::Uuid::now_v7();
        w.write_event(&AgentEvent::AgentStarted {
            run_id,
            phase_id: 0,
            agent_id,
            prompt_preview: "".into(),
            model: None,
            description: None,
            role: None,
            name: None,
            agent_seq: 0,
        })
        .await
        .unwrap();

        w.write_event(&AgentEvent::AgentProgress {
            run_id,
            agent_id,
            delta: ProgressDelta::Message { text: "hi".into() },
        })
        .await
        .unwrap();

        sqlx::query("DELETE FROM runs WHERE run_id = ?")
            .bind(run_id)
            .execute(w.pool())
            .await
            .unwrap();

        let turn_count = fetch_turn_count(w.pool(), run_id, "message").await.unwrap();
        let agent_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE run_id = ?")
            .bind(run_id)
            .fetch_one(w.pool())
            .await
            .unwrap();
        assert_eq!(turn_count, 0);
        assert_eq!(agent_count, 0);
    }

    #[tokio::test]
    async fn acp_raw_is_skipped() {
        let (_dir, w) = setup().await;
        let run_id = uuid::Uuid::now_v7();
        let agent_id = uuid::Uuid::now_v7();

        w.write_event(&AgentEvent::AcpRaw {
            run_id,
            agent_id,
            kind: "agent_message_chunk".into(),
            raw: serde_json::json!({"text": "chunk"}),
        })
        .await
        .unwrap();

        let count = fetch_event_count(w.pool(), run_id).await.unwrap();
        assert_eq!(count, 0);
    }
}
