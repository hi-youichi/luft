//! Query DTO types shared between the CLI and storage layers.
//!
//! The actual query functions now live in `luft-storage::reader` (SQLite-backed)
//! or in the `luft` main crate. This module retains only the data types.

use crate::state::RunCheckpoint;

/// Summary view of a run's checkpoint — the query DTO shared by the CLI.
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

pub enum ReportStatus {
    Found(serde_json::Value),
    NotFound,
    RunFinished,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AgentResultCache, CheckpointStatus, PhaseSummary};
    use std::collections::HashMap;

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
            agent_sessions: HashMap::new(),
            findings: vec![],
            total_tokens: 0,
            created_at: 1719000000,
            updated_at: 1719000100,
            workflow_meta: None,
            started_agent_ids: vec![],
        };
        let output = StatusOutput::from(("run_dir", &cp));
        assert_eq!(output.run_id, run_id.to_string());
        assert_eq!(output.run_dir, "run_dir");
        assert_eq!(output.task, "test task");
        assert_eq!(output.status, "running");
        assert_eq!(output.current_phase, 1);
    }

    #[test]
    fn status_output_with_completed_agents() {
        let run_id = uuid::Uuid::now_v7();
        let agent_id = uuid::Uuid::now_v7();
        let mut agent_results = HashMap::new();
        agent_results.insert(
            agent_id,
            AgentResultCache {
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
            },
        );
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
            agent_sessions: HashMap::new(),
            findings: vec![],
            total_tokens: 500,
            created_at: 1719000000,
            updated_at: 1719000100,
            workflow_meta: None,
            started_agent_ids: vec![agent_id],
        };
        let output = StatusOutput::from(("run_dir", &cp));
        assert_eq!(output.status, "completed");
        assert_eq!(output.completed_agents, 1);
        assert_eq!(output.total_tokens, 500);
    }
}
