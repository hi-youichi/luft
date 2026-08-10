//! In-memory `CheckpointBackend` for tests and lightweight usage.
//!
//! No file I/O, no SQLite — just `RwLock` state in memory.

use crate::contract::event::AgentEvent;
use crate::contract::finding::Finding;
use crate::contract::ids::RunId;
use crate::state::{
    AgentResultCache, AgentSessionCheckpoint, CheckpointBackend, CheckpointStatus, RunCheckpoint,
};
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Default)]
pub struct InMemoryBackend {
    checkpoint: RwLock<Option<RunCheckpoint>>,
    events: RwLock<Vec<AgentEvent>>,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CheckpointBackend for InMemoryBackend {
    fn init_run(&self, run_id: RunId, task: &str, _run_dir: &str) -> anyhow::Result<()> {
        let now = crate::state::current_timestamp();
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
        Ok(())
    }

    fn init_run_with_meta(
        &self,
        run_id: RunId,
        task: &str,
        _run_dir: &str,
        workflow_meta: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.init_run(run_id, task, "")?;
        if let Some(ref mut cp) = *self.checkpoint.write().unwrap() {
            cp.workflow_meta = Some(workflow_meta);
        }
        Ok(())
    }

    fn open_run(&self, run_id: RunId) -> anyhow::Result<Option<RunCheckpoint>> {
        let cp = self.checkpoint.read().unwrap().clone();
        Ok(cp.filter(|c| c.run_id == run_id))
    }

    fn append_event(&self, event: &AgentEvent) -> anyhow::Result<()> {
        self.events.write().unwrap().push(event.clone());
        Ok(())
    }

    fn upsert_agent_result(&self, cache: &AgentResultCache) -> anyhow::Result<()> {
        if let Some(ref mut cp) = *self.checkpoint.write().unwrap() {
            cp.agent_results.insert(cache.agent_id, cache.clone());
        }
        Ok(())
    }

    fn upsert_agent_session(&self, session: &AgentSessionCheckpoint) -> anyhow::Result<()> {
        if let Some(ref mut cp) = *self.checkpoint.write().unwrap() {
            cp.agent_sessions.insert(session.agent_id, session.clone());
        }
        Ok(())
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
        Ok(self.events.read().unwrap().clone())
    }

    fn can_resume(&self) -> bool {
        self.checkpoint
            .read()
            .unwrap()
            .as_ref()
            .map(|cp| {
                matches!(
                    cp.status,
                    CheckpointStatus::Running
                        | CheckpointStatus::Failed
                        | CheckpointStatus::Cancelled
                )
            })
            .unwrap_or(false)
    }

    fn reset_status_to_running(&self) -> anyhow::Result<()> {
        if let Some(ref mut cp) = *self.checkpoint.write().unwrap() {
            cp.status = CheckpointStatus::Running;
        }
        Ok(())
    }

    fn cancel(&self) -> anyhow::Result<()> {
        if let Some(ref mut cp) = *self.checkpoint.write().unwrap() {
            cp.status = CheckpointStatus::Cancelled;
        }
        Ok(())
    }

    fn save_checkpoint(&self, checkpoint: &RunCheckpoint) -> anyhow::Result<()> {
        *self.checkpoint.write().unwrap() = Some(checkpoint.clone());
        Ok(())
    }
}
