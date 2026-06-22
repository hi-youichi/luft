use crate::core::contract::backend::{AgentBackend, RunContext};
use crate::core::contract::event::{AgentEvent, RunStatus};
use crate::core::contract::ids::{RunId, TokenUsage};
use crate::core::journal::JournalStore;
use crate::core::run_dir::{compose, derive_slug, ensure_unique};
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

pub fn check_resumable(run_dir_name: &str, base_dir: &Path) -> ResumeCheck {
    let run_dir = base_dir.join(run_dir_name);
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
/// how it was requested.
pub struct RunSpec {
    pub run_id: RunId,
    /// Human-readable on-disk directory name (e.g. `deep-research_1781980050`).
    pub run_dir_name: String,
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
    let run_id = RunId::now_v7();
    Ok(RunSpec {
        run_id,
        script,
        task_label,
        resuming: false,
        extra_args: serde_json::json!({}),
        run_dir_name: String::new(),
    })
}

/// Assign the final run directory name once the base dir is known.
/// Called by the CLI after `resolve_fresh` so `ensure_unique` can scan the
/// filesystem.
pub fn assign_dir_name(spec: &mut RunSpec, base_dir: &Path) {
    let (wf, nl) = slug_sources(spec);
    let slug = derive_slug(wf, nl);
    let ts = spec
        .run_id
        .get_timestamp()
        .map(|t| t.to_unix().0)
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
    let dir_name = compose(&slug, ts);
    spec.run_dir_name = ensure_unique(base_dir, &dir_name);
}

fn slug_sources(spec: &RunSpec) -> (Option<&Path>, Option<&str>) {
    let label = &spec.task_label;
    if label.contains('.') && !label.contains(' ') {
        (Some(Path::new(label)), None)
    } else if label == "maestro workflow" {
        (None, None)
    } else {
        (None, Some(label))
    }
}

/// Resolve a resume of a specific run by reading its checkpoint + persisted
/// `workflow.lua`. Errors if the run is missing or has finished.
pub fn resolve_resume(run_dir_name: &str, base_dir: &Path) -> Result<RunSpec> {
    let run_dir = base_dir.join(run_dir_name);
    let content = std::fs::read_to_string(run_dir.join("checkpoint.json"))
        .map_err(|_| anyhow::anyhow!("run {} not found", run_dir_name))?;
    let checkpoint: RunCheckpoint = serde_json::from_str(&content)?;
    if matches!(
        checkpoint.status,
        CheckpointStatus::Completed | CheckpointStatus::Cancelled | CheckpointStatus::Failed
    ) {
        anyhow::bail!("run {} is not resumable (status: {:?})", run_dir_name, checkpoint.status);
    }
    let script = std::fs::read_to_string(run_dir.join("workflow.lua"))
        .map_err(|_| anyhow::anyhow!("workflow.lua not found in run directory {}", run_dir.display()))?;
    Ok(RunSpec {
        run_id: checkpoint.run_id,
        run_dir_name: run_dir_name.to_string(),
        script,
        task_label: checkpoint.task,
        resuming: true,
        extra_args: serde_json::json!({}),
    })
}

/// Find the most recent run that has a resumable checkpoint (CLI `--resume`
/// with no explicit run id). Status is validated later by [`resolve_resume`].
pub fn latest_resumable(base_dir: &Path) -> Result<String> {
    let run_dirs = list_runs(base_dir)?;
    run_dirs
        .iter()
        .rev()
        .find(|dir| base_dir.join(dir).join("checkpoint.json").exists())
        .cloned()
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
/// token: the CLI subscribes locally for headless output. Must be
/// called from within a tokio runtime (it spawns the forwarder and captures
/// `Handle::current()`).
pub fn prepare(
    spec: &RunSpec,
    backend: Arc<dyn AgentBackend>,
    base_dir: &Path,
    run_ctx: &RunContext,
) -> Result<PreparedRun> {
    let run_dir = base_dir.join(&spec.run_dir_name);

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
    let fwd_run_id = spec.run_id;
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                // Raw ACP events are a live observability stream, not durable
                // history — skip them so events.jsonl / get_logs stay lean.
                Ok(AgentEvent::AcpRaw { .. }) => {}
                Ok(evt) => {
                    if let Err(e) = store.append_event(&evt) {
                        tracing::warn!(run_id = %fwd_run_id, error = %e, "failed to persist event to journal");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(run_id = %fwd_run_id, skipped = n, "journal forwarder lagged; dropped events");
                }
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
/// event. Returns the report value (or the script error).
pub async fn execute(
    run_ctx: &RunContext,
    runtime: Runtime,
    script: String,
) -> Result<std::result::Result<serde_json::Value, ScriptError>> {
    let run_id = run_ctx.run_id;
    tracing::debug!(%run_id, "execute: spawning Lua script on blocking thread");
    // mlua is not Send-safe to drive from an async worker thread, and the SDK
    // primitives call Handle::block_on internally — both require a blocking
    // thread outside the async runtime.
    let result = match tokio::task::spawn_blocking(move || runtime.execute(&script))
        .await
    {
        Ok(r) => {
            tracing::debug!(%run_id, "execute: blocking thread returned");
            r
        }
        Err(e) => {
            tracing::error!(%run_id, error = %e, "execution task panicked");
            let _ = run_ctx.events.send(AgentEvent::RunDone {
                run_id,
                status: RunStatus::Failed,
                total_tokens: TokenUsage::default(),
                report: serde_json::Value::Null,
            });
            return Err(anyhow::anyhow!("execution task panicked: {}", e));
        }
    };

    let status = if result.is_ok() { RunStatus::Completed } else { RunStatus::Failed };
    if let Err(ref e) = result {
        tracing::warn!(%run_id, error = %e, "run finished with a script error");
    }
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
        let result = check_resumable("nonexistent_123", &temp_dir);
        assert!(matches!(result, ResumeCheck::NotFound));
    }

    #[test]
    fn resume_check_completed() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_completed");
        let dir_name = "test_123";
        let run_dir = temp_dir.join(dir_name);
        std::fs::create_dir_all(&run_dir).unwrap();

        let checkpoint = RunCheckpoint {
            run_id: RunId::now_v7(),
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

        let result = check_resumable(dir_name, &temp_dir);
        assert!(matches!(result, ResumeCheck::NotResumable(CheckpointStatus::Completed)));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn resume_check_running() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_running");
        let dir_name = "test_123";
        let run_dir = temp_dir.join(dir_name);
        std::fs::create_dir_all(&run_dir).unwrap();

        let checkpoint = RunCheckpoint {
            run_id: RunId::now_v7(),
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

        let result = check_resumable(dir_name, &temp_dir);
        assert!(matches!(result, ResumeCheck::CanResume));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn resume_check_no_checkpoint() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_nockpt");
        let dir_name = "test_123";
        let run_dir = temp_dir.join(dir_name);
        std::fs::create_dir_all(&run_dir).unwrap();

        let result = check_resumable(dir_name, &temp_dir);
        assert!(matches!(result, ResumeCheck::CanResume));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}