//! SQLite-backed query functions for the CLI.
//!
//! These functions replace the old file-based `luft_core::query` functions.
//! They open the shared `luft.db` from the base directory and query it
//! directly.

use luft_core::contract::event::AgentEvent;
use luft_core::contract::finding::Finding;
pub use luft_core::query::{ReportStatus, StatusOutput};
use luft_core::state::{CheckpointStatus, RunCheckpoint, PhaseSummary};
use luft_storage::DbPool;
use sqlx::Row;
use std::collections::HashMap;
use std::path::Path;

fn block_on_sql<F: std::future::Future>(f: F) -> F::Output {
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

async fn open_pool(base_dir: &Path) -> Result<DbPool, luft_storage::StorageError> {
    let db_path = base_dir.join(luft_storage::DEFAULT_DB_PATH);
    luft_storage::open_db(&db_path).await
}

/// Rebuild a full RunCheckpoint from SQLite, using a pool directly.
async fn fetch_checkpoint(pool: &DbPool, run_id: uuid::Uuid) -> anyhow::Result<Option<RunCheckpoint>> {
    let cp_row = sqlx::query(
        "SELECT status, current_phase, total_tokens, created_at, updated_at,
                workflow_meta, started_agent_ids
         FROM checkpoints WHERE run_id = ?",
    )
    .bind(run_id)
    .fetch_optional(pool)
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

    let task: String = sqlx::query_scalar("SELECT task FROM runs WHERE run_id = ?")
        .bind(run_id)
        .fetch_one(pool)
        .await?;

    let phase_rows = sqlx::query(
        "SELECT phase_id, label, planned, ok, failed, description, role
         FROM phases WHERE run_id = ? ORDER BY phase_id",
    )
    .bind(run_id)
    .fetch_all(pool)
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

    let agent_rows = sqlx::query(
        "SELECT agent_id, phase_id, status, output, findings_json,
                input_tokens + output_tokens as tokens,
                cache_key_hash, description, role, completed_at
         FROM agents WHERE run_id = ? AND status != 'running'",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;

    let mut agent_results: HashMap<uuid::Uuid, luft_core::state::AgentResultCache> = HashMap::new();
    for r in agent_rows {
        let agent_id_bytes: Vec<u8> = r.try_get("agent_id").unwrap_or_default();
        let agent_id = uuid::Uuid::from_slice(&agent_id_bytes).unwrap_or_default();
        let output_str: Option<String> = r.try_get("output").unwrap_or(None);
        let output: serde_json::Value = output_str
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null);
        let findings_str: String = r.try_get("findings_json").unwrap_or_else(|_| "[]".into());
        let findings: Vec<Finding> = serde_json::from_str(&findings_str).unwrap_or_default();

        agent_results.insert(
            agent_id,
            luft_core::state::AgentResultCache {
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

    let workflow_meta = workflow_meta.and_then(|s| serde_json::from_str(&s).ok());
    let started_agent_ids: Vec<uuid::Uuid> =
        serde_json::from_str(&started_agent_ids_json).unwrap_or_default();

    // Fetch findings
    let finding_rows = sqlx::query("SELECT data FROM findings WHERE run_id = ?")
        .bind(run_id)
        .fetch_all(pool)
        .await?;
    let findings: Vec<Finding> = finding_rows
        .into_iter()
        .filter_map(|r| {
            let data: String = r.try_get("data").ok()?;
            serde_json::from_str(&data).ok()
        })
        .collect();

    // Fetch agent sessions
    let session_rows = sqlx::query(
        "SELECT agent_id, backend_id, protocol_session_id, session_id, status, updated_at, resumable
         FROM agent_sessions WHERE run_id = ?",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;
    let mut agent_sessions: HashMap<uuid::Uuid, luft_core::state::AgentSessionCheckpoint> = HashMap::new();
    for r in session_rows {
        let agent_id_bytes: Vec<u8> = r.try_get("agent_id").unwrap_or_default();
        let agent_id = uuid::Uuid::from_slice(&agent_id_bytes).unwrap_or_default();
        agent_sessions.insert(
            agent_id,
            luft_core::state::AgentSessionCheckpoint {
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

    Ok(Some(RunCheckpoint {
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
    }))
}

/// List all runs as `StatusOutput` entries.
pub fn list_runs(base_dir: &Path) -> anyhow::Result<Vec<StatusOutput>> {
    block_on_sql(async {
        let pool = open_pool(base_dir).await?;
        let rows = sqlx::query(
            "SELECT c.run_id, c.status, c.current_phase, c.total_tokens, c.created_at, c.updated_at,
                    r.task, r.run_dir
             FROM checkpoints c
             JOIN runs r ON c.run_id = r.run_id
             ORDER BY c.created_at DESC",
        )
        .fetch_all(&pool)
        .await?;

        let mut outputs = Vec::new();
        for row in rows {
            let run_id_bytes: Vec<u8> = row.try_get("run_id").unwrap_or_default();
            let run_id = uuid::Uuid::from_slice(&run_id_bytes).unwrap_or_default();
            let status: String = row.try_get("status").unwrap_or_default();
            let task: String = row.try_get("task").unwrap_or_default();
            let run_dir: Option<String> = row.try_get("run_dir").unwrap_or(None);
            let current_phase: u32 = row.try_get::<i64, _>("current_phase").unwrap_or(0) as u32;
            let total_tokens: u64 = row.try_get::<i64, _>("total_tokens").unwrap_or(0) as u64;
            let created_at: u64 = row.try_get::<i64, _>("created_at").unwrap_or(0) as u64;
            let updated_at: u64 = row.try_get::<i64, _>("updated_at").unwrap_or(0) as u64;

            let created = chrono::DateTime::from_timestamp(created_at as i64, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            let updated = chrono::DateTime::from_timestamp(updated_at as i64, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();

            outputs.push(StatusOutput {
                run_id: run_id.to_string(),
                run_dir: run_dir.unwrap_or_else(|| run_id.to_string()),
                task,
                status,
                current_phase,
                completed_phases: 0,
                total_started: 0,
                completed_agents: 0,
                running_agents: 0,
                total_tokens,
                created_at: created,
                updated_at: updated,
            });
        }
        Ok(outputs)
    })
}

/// Get status for a specific run.
pub fn get_status(run_dir: &str, base_dir: &Path) -> anyhow::Result<Option<StatusOutput>> {
    match get_checkpoint(run_dir, base_dir)? {
        Some(cp) => Ok(Some(StatusOutput::from((run_dir, &cp)))),
        None => Ok(None),
    }
}

/// Get checkpoint for a specific run from the SQLite DB.
pub fn get_checkpoint(run_dir: &str, base_dir: &Path) -> anyhow::Result<Option<RunCheckpoint>> {
    block_on_sql(async {
        let pool = open_pool(base_dir).await?;

        // Primary lookup: by run_dir column
        let row = sqlx::query("SELECT run_id FROM runs WHERE run_dir = ?")
            .bind(run_dir)
            .fetch_optional(&pool)
            .await?;

        if let Some(row) = row {
            let run_id_bytes: Vec<u8> = sqlx::Row::try_get(&row, "run_id")?;
            let run_id = uuid::Uuid::from_slice(&run_id_bytes).unwrap_or_default();
            return fetch_checkpoint(&pool, run_id).await;        }

        // Fallback: try run_dir as UUID directly
        if let Ok(run_id) = uuid::Uuid::parse_str(run_dir) {
            return fetch_checkpoint(&pool, run_id).await;
        }

        // Last resort: scan all runs for a UUID prefix match
        let rows = sqlx::query("SELECT run_id FROM runs ORDER BY started_ts DESC")
            .fetch_all(&pool)
            .await?;

        for r in rows {
            let run_id_bytes: Vec<u8> = sqlx::Row::try_get(&r, "run_id").unwrap_or_default();
            let run_id = uuid::Uuid::from_slice(&run_id_bytes).unwrap_or_default();
            let id_str = run_id.to_string();
            if id_str.starts_with(run_dir) || run_dir.starts_with(&id_str) {
                if let Some(cp) = fetch_checkpoint(&pool, run_id).await? {
                    return Ok(Some(cp));
                }
            }
        }

        Ok(None)
    })
}

/// Get events for a run.
pub fn get_events(run_dir: &str, base_dir: &Path) -> anyhow::Result<Vec<AgentEvent>> {
    let cp = get_checkpoint(run_dir, base_dir)?
        .ok_or_else(|| anyhow::anyhow!("run not found: {}", run_dir))?;
    block_on_sql(async {
        let pool = open_pool(base_dir).await?;
        let rows = sqlx::query("SELECT payload FROM events WHERE run_id = ? ORDER BY seq")
            .bind(cp.run_id)
            .fetch_all(&pool)
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

/// Get findings for a run.
pub fn get_findings(run_dir: &str, base_dir: &Path) -> anyhow::Result<Vec<Finding>> {
    let cp = get_checkpoint(run_dir, base_dir)?
        .ok_or_else(|| anyhow::anyhow!("run not found: {}", run_dir))?;
    block_on_sql(async {
        let pool = open_pool(base_dir).await?;
        let rows = sqlx::query("SELECT data FROM findings WHERE run_id = ?")
            .bind(cp.run_id)
            .fetch_all(&pool)
            .await?;

        let findings: Vec<Finding> = rows
            .into_iter()
            .filter_map(|r| {
                let data: String = r.try_get("data").ok()?;
                serde_json::from_str(&data).ok()
            })
            .collect();
        Ok(findings)
    })
}

/// Cancel a run by updating its checkpoint status.
pub fn cancel_run(run_dir: &str, base_dir: &Path) -> anyhow::Result<()> {
    let cp = get_checkpoint(run_dir, base_dir)?
        .ok_or_else(|| anyhow::anyhow!("run not found: {}", run_dir))?;
    block_on_sql(async {
        let pool = open_pool(base_dir).await?;
        sqlx::query(
            "UPDATE checkpoints SET status = 'cancelled', updated_at = ? WHERE run_id = ? AND status = 'running'",
        )
        .bind(luft_core::state::current_timestamp() as i64)
        .bind(cp.run_id)
        .execute(&pool)
        .await?;
        Ok(())
    })
}

/// Get the report for a run (from RunDone event).
pub fn get_report(run_dir: &str, base_dir: &Path) -> anyhow::Result<ReportStatus> {
    let events = get_events(run_dir, base_dir)?;
    for event in events.iter().rev() {
        if let AgentEvent::RunDone { report, .. } = event {
            if !report.is_null() {
                return Ok(ReportStatus::Found(report.clone()));
            }
            return Ok(ReportStatus::RunFinished);
        }
    }
    Ok(ReportStatus::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn base_dir() -> TempDir {
        tempfile::tempdir().expect("create query test directory")
    }

    #[test]
    fn list_runs_returns_empty_for_new_database() {
        let dir = base_dir();

        let runs = list_runs(dir.path()).expect("query empty database");

        assert!(runs.is_empty());
    }

    #[test]
    fn missing_run_returns_no_status_or_checkpoint() {
        let dir = base_dir();

        assert!(get_status("missing", dir.path()).unwrap().is_none());
        assert!(get_checkpoint("missing", dir.path()).unwrap().is_none());
    }

    #[test]
    fn missing_run_errors_when_querying_events_or_findings() {
        let dir = base_dir();

        assert!(get_events("missing", dir.path()).is_err());
        assert!(get_findings("missing", dir.path()).is_err());
    }

    #[test]
    fn missing_run_report_returns_not_found_error() {
        let dir = base_dir();

        match get_report("missing", dir.path()) {
            Ok(_) => panic!("missing run should return an error"),
            Err(error) => assert!(error.to_string().contains("run not found: missing")),
        }
    }
}
