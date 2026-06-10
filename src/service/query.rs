use crate::core::contract::event::AgentEvent;
use crate::core::contract::finding::Finding;
use crate::core::contract::ids::RunId;
use crate::core::state::{get_run_store, list_runs as list_run_ids};
use crate::cli::StatusOutput;
use anyhow::Result;
use std::path::Path;

pub fn list_runs(base_dir: &Path) -> Result<Vec<StatusOutput>> {
    let run_ids = list_run_ids(base_dir)?;
    let mut outputs = Vec::new();
    for rid in run_ids {
        if let Ok(store) = get_run_store(rid, base_dir) {
            if let Some(checkpoint) = store.get_checkpoint() {
                outputs.push(StatusOutput::from(&checkpoint));
            }
        }
    }
    outputs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(outputs)
}

pub fn get_status(run_id: RunId, base_dir: &Path) -> Result<Option<StatusOutput>> {
    let store = get_run_store(run_id, base_dir)?;
    if let Some(checkpoint) = store.get_checkpoint() {
        Ok(Some(StatusOutput::from(&checkpoint)))
    } else {
        Ok(None)
    }
}

pub fn get_logs(run_id: RunId, base_dir: &Path, limit: Option<usize>) -> Result<Vec<String>> {
    let store = get_run_store(run_id, base_dir)?;
    let events = store.get_event_log()?;
    let logs: Vec<String> = events
        .into_iter()
        .take(limit.unwrap_or(1000))
        .map(|e| serde_json::to_string(&e).unwrap_or_default())
        .collect();
    Ok(logs)
}

pub fn get_findings(run_id: RunId, base_dir: &Path) -> Result<Vec<Finding>> {
    let store = get_run_store(run_id, base_dir)?;
    Ok(store.get_findings())
}

pub fn cancel_run(run_id: RunId, base_dir: &Path) -> Result<()> {
    let store = get_run_store(run_id, base_dir)?;
    store.cancel()?;
    Ok(())
}

pub enum ReportStatus {
    Found(serde_json::Value),
    NotFound,
    RunFinished,
}

pub fn get_report(run_id: RunId, base_dir: &Path) -> Result<ReportStatus> {
    let run_dir = base_dir.join(run_id.to_string());
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
        if let Ok(evt) = serde_json::from_str::<AgentEvent>(line) {
            if let AgentEvent::RunDone { report, .. } = evt {
                return Ok(ReportStatus::Found(report));
            }
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
        let run_id = RunId::now_v7();
        let result = get_status(run_id, &temp_dir);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_report_not_found_no_dir() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_report");
        let run_id = RunId::now_v7();
        let result = get_report(run_id, &temp_dir).unwrap();
        assert!(matches!(result, ReportStatus::NotFound));
    }

    #[test]
    fn get_report_run_finished_no_events() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_report2");
        let run_id = RunId::now_v7();
        let run_dir = temp_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();
        let result = get_report(run_id, &temp_dir).unwrap();
        assert!(matches!(result, ReportStatus::RunFinished));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_report_with_run_done() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_report3");
        let run_id = RunId::now_v7();
        let run_dir = temp_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();
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
        ).unwrap();
        let result = get_report(run_id, &temp_dir).unwrap();
        match result {
            ReportStatus::Found(data) => assert_eq!(data, report),
            _ => panic!("expected Found"),
        }
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_report_no_run_done_event() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_report4");
        let run_id = RunId::now_v7();
        let run_dir = temp_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();
        let evt = AgentEvent::RunStarted {
            run_id,
            task: "t".into(),
            ts: Utc::now(),
        };
        std::fs::write(
            run_dir.join("events.jsonl"),
            serde_json::to_string(&evt).unwrap(),
        ).unwrap();
        let result = get_report(run_id, &temp_dir).unwrap();
        assert!(matches!(result, ReportStatus::NotFound));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}