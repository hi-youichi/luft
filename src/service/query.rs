use crate::core::contract::event::AgentEvent;
use crate::core::contract::finding::Finding;
use crate::core::state::{get_run_store, list_runs as list_run_dirs, RunCheckpoint};
use anyhow::Result;
use std::path::Path;

/// Summary view of a run's checkpoint — the query DTO shared by the CLI.
/// It lives in the query layer (not a presentation layer) so that
/// the binary `commands` depend downward on `service`, not the reverse.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StatusOutput {
    pub run_id: String,
    pub run_dir: String,
    pub task: String,
    pub status: String,
    pub current_phase: u32,
    pub completed_phases: usize,
    pub total_agents: usize,
    pub completed_agents: usize,
    pub total_tokens: u64,
    pub created_at: String,
    pub updated_at: String,
}

impl From<(&str, &RunCheckpoint)> for StatusOutput {
    fn from((run_dir, cp): (&str, &RunCheckpoint)) -> Self {
        let created = chrono::DateTime::from_timestamp(cp.created_at as i64, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();
        let updated = chrono::DateTime::from_timestamp(cp.updated_at as i64, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();

        Self {
            run_id: cp.run_id.to_string(),
            run_dir: run_dir.to_string(),
            task: cp.task.clone(),
            status: format!("{:?}", cp.status).to_lowercase(),
            current_phase: cp.current_phase,
            completed_phases: cp.completed_phases.len(),
            total_agents: cp.agent_results.len(),
            completed_agents: cp
                .agent_results
                .values()
                .filter(|r| r.status == "ok")
                .count(),
            total_tokens: cp.total_tokens,
            created_at: created,
            updated_at: updated,
        }
    }
}

/// Fetch a run's checkpoint, guarding against unknown run dirs: returns
/// `Ok(None)` rather than letting `get_run_store` create the run directory for
/// an id that was never started. The single existence-checked accessor shared
/// by `list_runs` / `get_status` and the binary `status` command.
pub fn get_checkpoint(run_dir_name: &str, base_dir: &Path) -> Result<Option<RunCheckpoint>> {
    if !base_dir.join(run_dir_name).exists() {
        return Ok(None);
    }
    let store = get_run_store(run_dir_name, base_dir)?;
    Ok(store.get_checkpoint())
}

pub fn list_runs(base_dir: &Path) -> Result<Vec<StatusOutput>> {
    let run_dirs = list_run_dirs(base_dir)?;
    let mut outputs = Vec::new();
    for dir_name in run_dirs {
        if let Ok(Some(checkpoint)) = get_checkpoint(&dir_name, base_dir) {
            outputs.push(StatusOutput::from((&dir_name[..], &checkpoint)));
        }
    }
    outputs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(outputs)
}

pub fn get_status(run_dir_name: &str, base_dir: &Path) -> Result<Option<StatusOutput>> {
    Ok(get_checkpoint(run_dir_name, base_dir)?
        .as_ref()
        .map(|cp| StatusOutput::from((run_dir_name, cp))))
}

/// Raw event log for a run (chronological). Callers own any slicing/formatting.
pub fn get_events(run_dir_name: &str, base_dir: &Path) -> Result<Vec<AgentEvent>> {
    let store = get_run_store(run_dir_name, base_dir)?;
    Ok(store.get_event_log()?)
}

pub fn get_logs(run_dir_name: &str, base_dir: &Path, limit: Option<usize>) -> Result<Vec<String>> {
    let logs: Vec<String> = get_events(run_dir_name, base_dir)?
        .into_iter()
        .take(limit.unwrap_or(1000))
        .map(|e| serde_json::to_string(&e).unwrap_or_default())
        .collect();
    Ok(logs)
}

pub fn get_findings(run_dir_name: &str, base_dir: &Path) -> Result<Vec<Finding>> {
    let store = get_run_store(run_dir_name, base_dir)?;
    Ok(store.get_findings())
}

pub fn cancel_run(run_dir_name: &str, base_dir: &Path) -> Result<()> {
    let store = get_run_store(run_dir_name, base_dir)?;
    store.cancel()?;
    Ok(())
}

pub enum ReportStatus {
    Found(serde_json::Value),
    NotFound,
    RunFinished,
}

pub fn get_report(run_dir_name: &str, base_dir: &Path) -> Result<ReportStatus> {
    let run_dir = base_dir.join(run_dir_name);
    let events_path = run_dir.join("events.jsonl");
    if !events_path.exists() {
        if run_dir.exists() {
            return Ok(ReportStatus::RunFinished);
        } else {
            return Ok(ReportStatus::NotFound);
        }
    }
    let content = std::fs::read_to_string(&events_path)?;
    for line in content.lines().rev() {
        if let Ok(AgentEvent::RunDone { report, .. }) = serde_json::from_str::<AgentEvent>(line) {
            return Ok(ReportStatus::Found(report));
        }
    }
    Ok(ReportStatus::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::event::RunStatus;
    use crate::core::contract::ids::TokenUsage;

    use chrono::Utc;

    #[test]
    fn get_status_non_existing_run() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_status_nonexist");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let result = get_status("nonexistent_123", &temp_dir);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_report_not_found_no_dir() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_report");
        let result = get_report("nonexistent_123", &temp_dir).unwrap();
        assert!(matches!(result, ReportStatus::NotFound));
    }

    #[test]
    fn get_report_run_finished_no_events() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_report2");
        let dir_name = "test_123";
        let run_dir = temp_dir.join(dir_name);
        std::fs::create_dir_all(&run_dir).unwrap();
        let result = get_report(dir_name, &temp_dir).unwrap();
        assert!(matches!(result, ReportStatus::RunFinished));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_report_with_run_done() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_report3");
        let dir_name = "test_123";
        let run_dir = temp_dir.join(dir_name);
        std::fs::create_dir_all(&run_dir).unwrap();
        let run_id = uuid::Uuid::now_v7();
        let report = serde_json::json!({"summary": "done"});
        let evt = AgentEvent::RunDone {
            run_id,
            status: RunStatus::Completed,
            total_tokens: TokenUsage::default(),
            report: report.clone(),
        };
        std::fs::write(
            run_dir.join("events.jsonl"),
            serde_json::to_string(&evt).unwrap(),
        )
        .unwrap();
        let result = get_report(dir_name, &temp_dir).unwrap();
        match result {
            ReportStatus::Found(data) => assert_eq!(data, report),
            _ => panic!("expected Found"),
        }
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_report_no_run_done_event() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_report4");
        let dir_name = "test_123";
        let run_dir = temp_dir.join(dir_name);
        std::fs::create_dir_all(&run_dir).unwrap();
        let run_id = uuid::Uuid::now_v7();
        let evt = AgentEvent::RunStarted {
            run_id,
            task: "t".into(),
            ts: Utc::now(),
        };
        std::fs::write(
            run_dir.join("events.jsonl"),
            serde_json::to_string(&evt).unwrap(),
        )
        .unwrap();
        let result = get_report(dir_name, &temp_dir).unwrap();
        assert!(matches!(result, ReportStatus::NotFound));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
