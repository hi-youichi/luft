use crate::core::contract::backend::{AgentBackend, RunContext};
use crate::core::contract::event::{AgentEvent, RunStatus};
use crate::core::contract::ids::{RunId, TokenUsage};
use crate::core::journal::JournalStore;
use crate::core::scheduler::{BackendRegistry, Scheduler, SchedulerConfig};
use crate::core::state::{list_runs, CheckpointStatus, RunCheckpoint};
use crate::runtime::{ExecLimits, Runtime, ScriptError};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct RunInput {
    pub nl: Option<String>,
    pub workflow: Option<PathBuf>,
    pub script: Option<String>,
}

pub enum ValidateSourceError {
    NoneProvided,
    MultipleProvided,
}

pub fn validate_source(input: &RunInput) -> std::result::Result<(), ValidateSourceError> {
    let count = input.nl.is_some() as usize
        + input.workflow.is_some() as usize
        + input.script.is_some() as usize;
    match count {
        0 => Err(ValidateSourceError::NoneProvided),
        1 => Ok(()),
        _ => Err(ValidateSourceError::MultipleProvided),
    }
}

/// How a run's Lua script is sourced before execution. Resolved into the final
/// script string by [`resolve_script`]; the three variants are mutually
/// exclusive (enforced upstream by [`validate_source`]).
#[derive(Clone, Copy)]
pub enum ScriptSource<'a> {
    /// Natural-language task description — planned into a workflow by the planner.
    Nl(&'a str),
    /// Path to a workflow file on disk — read verbatim.
    Workflow(&'a Path),
    /// An inline Lua script — passed through as-is.
    Script(&'a str),
}

pub async fn resolve_script(
    source: ScriptSource<'_>,
    backend: Arc<dyn crate::core::contract::backend::AgentBackend>,
) -> Result<String> {
    match source {
        ScriptSource::Nl(nl) => {
            let cfg = crate::planner::PlannerConfig::default();
            let planned = crate::planner::plan_workflow(nl, backend, &cfg).await?;
            Ok(planned.script)
        }
        ScriptSource::Workflow(path) => {
            std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("failed to read workflow file: {}", e))
        }
        ScriptSource::Script(s) => Ok(s.to_string()),
    }
}

pub enum ResumeCheck {
    CanResume,
    NotFound,
    NotResumable(CheckpointStatus),
}

pub fn check_resumable(run_id: RunId, base_dir: &Path) -> ResumeCheck {
    let run_dir = base_dir.join(run_id.to_string());
    if !run_dir.exists() {
        return ResumeCheck::NotFound;
    }

    // Read only the `status` field, so a partially-written checkpoint still
    // gates correctly (a finished run is never silently treated as resumable).
    let checkpoint_path = run_dir.join("checkpoint.json");
    if let Ok(content) = std::fs::read_to_string(&checkpoint_path) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(status) = value
                .get("status")
                .and_then(|s| serde_json::from_value::<CheckpointStatus>(s.clone()).ok())
            {
                if matches!(
                    status,
                    CheckpointStatus::Completed | CheckpointStatus::Cancelled | CheckpointStatus::Failed
                ) {
                    return ResumeCheck::NotResumable(status);
                }
            }
        }
    }

    ResumeCheck::CanResume
}

/// A fully-resolved run: everything needed to prepare execution, regardless of
/// how it was requested (CLI args vs WS payload).
pub struct RunSpec {
    pub run_id: RunId,
    pub script: String,
    pub task_label: String,
    pub resuming: bool,
    /// JSON arguments passed to the workflow (the Lua `args` global).
    pub extra_args: serde_json::Value,
}

/// Resolve a fresh run from a script source: plans NL / reads a workflow file /
/// passes a script through, generates a new run id, and derives a task label.
/// Callers may override `task_label` / `extra_args` on the returned spec.
pub async fn resolve_fresh(
    source: ScriptSource<'_>,
    backend: Arc<dyn AgentBackend>,
) -> Result<RunSpec> {
    let task_label = match source {
        ScriptSource::Nl(nl) => nl.to_string(),
        ScriptSource::Workflow(p) => p.display().to_string(),
        ScriptSource::Script(_) => "maestro workflow".to_string(),
    };
    let script = resolve_script(source, backend).await?;
    Ok(RunSpec {
        run_id: RunId::now_v7(),
        script,
        task_label,
        resuming: false,
        extra_args: serde_json::json!({}),
    })
}

/// Resolve a resume of a specific run by reading its checkpoint + persisted
/// `workflow.lua`. Errors if the run is missing or has finished.
pub fn resolve_resume(run_id: RunId, base_dir: &Path) -> Result<RunSpec> {
    let run_dir = base_dir.join(run_id.to_string());
    let content = std::fs::read_to_string(run_dir.join("checkpoint.json"))
        .map_err(|_| anyhow::anyhow!("run {} not found", run_id))?;
    let checkpoint: RunCheckpoint = serde_json::from_str(&content)?;
    if matches!(
        checkpoint.status,
        CheckpointStatus::Completed | CheckpointStatus::Cancelled | CheckpointStatus::Failed
    ) {
        anyhow::bail!("run {} is not resumable (status: {:?})", run_id, checkpoint.status);
    }
    let script = std::fs::read_to_string(run_dir.join("workflow.lua"))
        .map_err(|_| anyhow::anyhow!("workflow.lua not found in run directory {}", run_dir.display()))?;
    Ok(RunSpec {
        run_id,
        script,
        task_label: checkpoint.task,
        resuming: true,
        extra_args: serde_json::json!({}),
    })
}

/// Find the most recent run that has a resumable checkpoint (CLI `--resume`
/// with no explicit run id). Status is validated later by [`resolve_resume`].
pub fn latest_resumable(base_dir: &Path) -> Result<RunId> {
    let run_ids = list_runs(base_dir)?;
    run_ids
        .iter()
        .rev()
        .copied()
        .find(|&rid| base_dir.join(rid.to_string()).join("checkpoint.json").exists())
        .ok_or_else(|| anyhow::anyhow!("no resumable run found"))
}

/// A prepared run: the Lua runtime plus the journal handle (single persistence
/// instance for the run).
pub struct PreparedRun {
    pub runtime: Runtime,
    pub journal: Arc<JournalStore>,
}

/// Build the per-run orchestration — journal, scheduler, event→journal
/// forwarder, and the Lua runtime — without executing it.
///
/// `run_ctx` is supplied by the caller, which owns the event channel + cancel
/// token: the CLI subscribes locally for TUI/headless output; the WS layer
/// stores them in its `RunHandle` for streaming and cancellation. Must be
/// called from within a tokio runtime (it spawns the forwarder and captures
/// `Handle::current()`).
pub fn prepare(
    spec: &RunSpec,
    backend: Arc<dyn AgentBackend>,
    base_dir: &Path,
    run_ctx: &RunContext,
) -> Result<PreparedRun> {
    let run_dir = base_dir.join(spec.run_id.to_string());

    // Journal: fresh runs init + persist the script (so they can be resumed);
    // resume runs open the journal to replay cached agents.
    let journal = Arc::new(
        JournalStore::new(&run_dir).map_err(|e| anyhow::anyhow!("failed to open journal: {}", e))?,
    );
    if spec.resuming {
        journal
            .open(spec.run_id)
            .map_err(|e| anyhow::anyhow!("failed to open journal for resume: {}", e))?;
    } else {
        journal
            .init_run(spec.run_id, &spec.task_label)
            .map_err(|e| anyhow::anyhow!("failed to init journal: {}", e))?;
        std::fs::write(run_dir.join("workflow.lua"), &spec.script)?;
    }

    // Scheduler. Journaling is handled inside the runtime (cache-key aware), so
    // no scheduler-level callback is required.
    let registry = BackendRegistry::new().with(backend);
    let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
    scheduler.init_run_with(spec.run_id, run_ctx.events.clone());

    // Forward the scheduler event stream into the journal's run store — the
    // single persistence instance for this run (avoids split-brain checkpoints).
    let store = journal.store();
    let mut rx = run_ctx.events.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(evt) => {
                    let _ = store.append_event(&evt);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(_) => continue,
            }
        }
    });

    // Capture the runtime handle here (async context) for the blocking executor.
    let handle = tokio::runtime::Handle::current();
    let runtime = Runtime::new(
        scheduler,
        run_ctx.clone(),
        spec.extra_args.clone(),
        ExecLimits::default(),
        Some(journal.clone()),
        handle,
    )?;

    Ok(PreparedRun { runtime, journal })
}

/// Execute the Lua runtime on a blocking thread and emit a terminal `RunDone`
/// event. Returns the report value (or the script error). Presentation-free, so
/// both the CLI and WS layers share it.
pub async fn execute(
    run_ctx: &RunContext,
    runtime: Runtime,
    script: String,
) -> Result<std::result::Result<serde_json::Value, ScriptError>> {
    let run_id = run_ctx.run_id;
    // mlua is not Send-safe to drive from an async worker thread, and the SDK
    // primitives call Handle::block_on internally — both require a blocking
    // thread outside the async runtime.
    let result = tokio::task::spawn_blocking(move || runtime.execute(&script))
        .await
        .map_err(|e| anyhow::anyhow!("execution task panicked: {}", e))?;

    let status = if result.is_ok() { RunStatus::Completed } else { RunStatus::Failed };
    let report = result.as_ref().ok().cloned().unwrap_or(serde_json::Value::Null);
    let _ = run_ctx.events.send(AgentEvent::RunDone {
        run_id,
        status,
        total_tokens: TokenUsage::default(),
        report,
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn validate_source_none() {
        let input = RunInput { nl: None, workflow: None, script: None };
        assert!(matches!(
            validate_source(&input),
            Err(ValidateSourceError::NoneProvided)
        ));
    }

    #[test]
    fn validate_source_multiple() {
        let input = RunInput {
            nl: Some("hi".into()),
            workflow: Some(PathBuf::from("wf.lua")),
            script: None,
        };
        assert!(matches!(
            validate_source(&input),
            Err(ValidateSourceError::MultipleProvided)
        ));
    }

    #[test]
    fn validate_source_nl_only() {
        let input = RunInput { nl: Some("hi".into()), workflow: None, script: None };
        assert!(validate_source(&input).is_ok());
    }

    #[test]
    fn validate_source_workflow_only() {
        let input = RunInput { nl: None, workflow: Some(PathBuf::from("wf.lua")), script: None };
        assert!(validate_source(&input).is_ok());
    }

    #[test]
    fn validate_source_script_only() {
        let input = RunInput { nl: None, workflow: None, script: Some("print(1)".into()) };
        assert!(validate_source(&input).is_ok());
    }

    #[test]
    fn resume_check_not_found() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_notfound");
        let run_id = RunId::now_v7();
        let result = check_resumable(run_id, &temp_dir);
        assert!(matches!(result, ResumeCheck::NotFound));
    }

    #[test]
    fn resume_check_completed() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_completed");
        let run_id = RunId::now_v7();
        let run_dir = temp_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();

        let checkpoint = RunCheckpoint {
            run_id,
            task: "t".into(),
            status: CheckpointStatus::Completed,
            current_phase: 1,
            completed_phases: vec![],
            agent_results: HashMap::new(),
            findings: vec![],
            total_tokens: 0,
            created_at: 0,
            updated_at: 0,
        };
        std::fs::write(
            run_dir.join("checkpoint.json"),
            serde_json::to_string(&checkpoint).unwrap(),
        ).unwrap();

        let result = check_resumable(run_id, &temp_dir);
        assert!(matches!(result, ResumeCheck::NotResumable(CheckpointStatus::Completed)));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn resume_check_running() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_running");
        let run_id = RunId::now_v7();
        let run_dir = temp_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();

        let checkpoint = RunCheckpoint {
            run_id,
            task: "t".into(),
            status: CheckpointStatus::Running,
            current_phase: 1,
            completed_phases: vec![],
            agent_results: HashMap::new(),
            findings: vec![],
            total_tokens: 0,
            created_at: 0,
            updated_at: 0,
        };
        std::fs::write(
            run_dir.join("checkpoint.json"),
            serde_json::to_string(&checkpoint).unwrap(),
        ).unwrap();

        let result = check_resumable(run_id, &temp_dir);
        assert!(matches!(result, ResumeCheck::CanResume));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn resume_check_no_checkpoint() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_nockpt");
        let run_id = RunId::now_v7();
        let run_dir = temp_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();

        let result = check_resumable(run_id, &temp_dir);
        assert!(matches!(result, ResumeCheck::CanResume));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}