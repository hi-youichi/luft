use luft_core::contract::event::AgentEvent;
use luft_core::contract::finding::Finding;
use luft_core::state::{get_run_store, list_runs as list_run_dirs, RunCheckpoint};
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
    pub total_started: usize,
    pub completed_agents: usize,
    pub running_agents: usize,
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
            total_started: cp.started_agent_ids.len(),
            completed_agents: cp.agent_results.len(),
            running_agents: cp
                .started_agent_ids
                .len()
                .saturating_sub(cp.agent_results.len()),
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
    use luft_core::contract::event::RunStatus;
    use luft_core::contract::ids::TokenUsage;

    use chrono::Utc;
    use std::collections::HashMap;
    use luft_core::state::{AgentResultCache, CheckpointStatus, PhaseSummary};
    use luft_core::contract::event::LogLevel;
    use luft_core::contract::finding::Severity;

    #[test]
    fn get_status_non_existing_run() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_status_nonexist");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let result = get_status("nonexistent_123", &temp_dir);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_report_not_found_no_dir() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_report");
        let result = get_report("nonexistent_123", &temp_dir).unwrap();
        assert!(matches!(result, ReportStatus::NotFound));
    }

    #[test]
    fn get_report_run_finished_no_events() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_report2");
        let dir_name = "test_123";
        let run_dir = temp_dir.join(dir_name);
        std::fs::create_dir_all(&run_dir).unwrap();
        let result = get_report(dir_name, &temp_dir).unwrap();
        assert!(matches!(result, ReportStatus::RunFinished));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_report_with_run_done() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_report3");
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
        ts: chrono::Utc::now(),
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
        let temp_dir = std::env::temp_dir().join("luft_svc_test_report4");
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

    #[test]
    fn status_output_from_checkpoint() {
        let run_id = uuid::Uuid::now_v7();
        let cp = RunCheckpoint {
            run_id,
            task: "test task".into(),
            status: CheckpointStatus::Running,
            current_phase: 1,
            completed_phases: vec![],
            agent_results: HashMap::new(),
            findings: vec![],
            total_tokens: 0,
            created_at: 1719000000,
            updated_at: 1719000100,
            completed_spans: vec![],
            workflow_meta: None,
            started_agent_ids: vec![],
        };
        let output = StatusOutput::from(("run_dir", &cp));
        assert_eq!(output.run_id, run_id.to_string());
        assert_eq!(output.run_dir, "run_dir");
        assert_eq!(output.task, "test task");
        assert_eq!(output.status, "running");
        assert_eq!(output.current_phase, 1);
        assert_eq!(output.completed_phases, 0);
        assert_eq!(output.total_started, 0);
        assert_eq!(output.completed_agents, 0);
        assert_eq!(output.running_agents, 0);
        assert_eq!(output.total_tokens, 0);
        assert!(!output.created_at.is_empty());
        assert!(!output.updated_at.is_empty());
    }

    #[test]
    fn status_output_with_completed_agents() {
        let run_id = uuid::Uuid::now_v7();
        let agent_id = uuid::Uuid::now_v7();
        let mut agent_results = HashMap::new();
        agent_results.insert(agent_id, AgentResultCache {
            agent_id,
            phase_id: 1,
            status: "ok".into(),
            output: serde_json::json!({}),
            findings: vec![],
            tokens: 500,
            completed_at: 1719000100,
            cache_key_hash: None,
            description: None,
            role: None,
        });
        let cp = RunCheckpoint {
            run_id,
            task: "task".into(),
            status: CheckpointStatus::Completed,
            current_phase: 2,
            completed_phases: vec![PhaseSummary {
                phase_id: 1,
                label: "phase 1".into(),
                planned: 1,
                ok: 1,
                failed: 0,
                description: None,
                role: None,
            }],
            agent_results,
            findings: vec![],
            total_tokens: 500,
            created_at: 1719000000,
            updated_at: 1719000100,
            completed_spans: vec![],
            workflow_meta: None,
            started_agent_ids: vec![agent_id],
        };
        let output = StatusOutput::from(("run_dir", &cp));
        assert_eq!(output.status, "completed");
        assert_eq!(output.current_phase, 2);
        assert_eq!(output.completed_phases, 1);
        assert_eq!(output.total_started, 1);
        assert_eq!(output.completed_agents, 1);
        assert_eq!(output.running_agents, 0);
        assert_eq!(output.total_tokens, 500);
    }

    #[test]
    fn get_checkpoint_existing_run() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_checkpoint");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let dir_name = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7();
        let store = get_run_store(&dir_name, &temp_dir).unwrap();
        store.init_run(run_id, "test task").unwrap();
        let result = get_checkpoint(&dir_name, &temp_dir).unwrap();
        assert!(result.is_some());
        let cp = result.unwrap();
        assert_eq!(cp.run_id, run_id);
        assert_eq!(cp.task, "test task");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_status_existing_run() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_status_exist");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let dir_name = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7();
        let store = get_run_store(&dir_name, &temp_dir).unwrap();
        store.init_run(run_id, "test task").unwrap();
        let result = get_status(&dir_name, &temp_dir).unwrap();
        assert!(result.is_some());
        let status = result.unwrap();
        assert_eq!(status.run_id, run_id.to_string());
        assert_eq!(status.task, "test task");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn list_runs_empty() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_list_empty");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let results = list_runs(&temp_dir).unwrap();
        assert!(results.is_empty());
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn list_runs_with_runs() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_list_runs");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let dir_a = uuid::Uuid::now_v7().to_string();
        let dir_b = uuid::Uuid::now_v7().to_string();
        let run_id_a = uuid::Uuid::now_v7();
        let store_a = get_run_store(&dir_a, &temp_dir).unwrap();
        store_a.init_run(run_id_a, "task a").unwrap();
        let run_id_b = uuid::Uuid::now_v7();
        let store_b = get_run_store(&dir_b, &temp_dir).unwrap();
        store_b.init_run(run_id_b, "task b").unwrap();
        let results = list_runs(&temp_dir).unwrap();
        assert_eq!(results.len(), 2);
        let ids: Vec<String> = results.iter().map(|r| r.run_id.clone()).collect();
        assert!(ids.contains(&run_id_a.to_string()));
        assert!(ids.contains(&run_id_b.to_string()));
        assert!(results[0].updated_at >= results[1].updated_at);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_events_success() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_events");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let dir_name = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7();
        let store = get_run_store(&dir_name, &temp_dir).unwrap();
        store.init_run(run_id, "task").unwrap();
        let event = AgentEvent::Log {
            run_id,
            agent_id: None,
            level: LogLevel::Info,
            msg: "test log".into(),
        };
        store.append_event(&event).unwrap();
        let events = get_events(&dir_name, &temp_dir).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], AgentEvent::Log { .. }));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_logs_without_limit() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_logs_nolimit");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let dir_name = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7();
        let store = get_run_store(&dir_name, &temp_dir).unwrap();
        store.init_run(run_id, "task").unwrap();
        for i in 0..3 {
            store.append_event(&AgentEvent::Log {
                run_id,
                agent_id: None,
                level: LogLevel::Info,
                msg: format!("log {}", i),
            }).unwrap();
        }
        let logs = get_logs(&dir_name, &temp_dir, None).unwrap();
        assert_eq!(logs.len(), 3);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_logs_with_limit() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_logs_limit");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let dir_name = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7();
        let store = get_run_store(&dir_name, &temp_dir).unwrap();
        store.init_run(run_id, "task").unwrap();
        for i in 0..5 {
            store.append_event(&AgentEvent::Log {
                run_id,
                agent_id: None,
                level: LogLevel::Info,
                msg: format!("log {}", i),
            }).unwrap();
        }
        let logs = get_logs(&dir_name, &temp_dir, Some(2)).unwrap();
        assert_eq!(logs.len(), 2);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn get_findings_success() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_findings");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let dir_name = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7();
        let store = get_run_store(&dir_name, &temp_dir).unwrap();
        store.init_run(run_id, "task").unwrap();
        let finding = Finding {
            kind: "test_kind".into(),
            severity: Severity::High,
            title: "Test Finding".into(),
            detail: "A detailed finding description".into(),
            location: None,
            evidence: vec![],
            data: serde_json::json!({}),
        };
        let mut cp = store.get_checkpoint().unwrap();
        cp.findings.push(finding);
        store.save_checkpoint(&cp).unwrap();
        let findings = get_findings(&dir_name, &temp_dir).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "test_kind");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn cancel_run_success() {
        let temp_dir = std::env::temp_dir().join("luft_svc_test_cancel");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let dir_name = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7();
        let store = get_run_store(&dir_name, &temp_dir).unwrap();
        store.init_run(run_id, "task").unwrap();
        cancel_run(&dir_name, &temp_dir).unwrap();
        let cp = get_checkpoint(&dir_name, &temp_dir).unwrap().unwrap();
        assert_eq!(cp.status, CheckpointStatus::Cancelled);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

}