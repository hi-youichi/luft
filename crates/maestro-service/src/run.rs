use maestro_core::contract::backend::{AgentBackend, RunContext};
use maestro_core::contract::event::{AgentEvent, RunStatus};
use maestro_core::contract::ids::{RunId, TokenUsage};
use maestro_core::journal::JournalStore;
use maestro_core::run_dir::{compose, derive_slug, ensure_unique};
use maestro_core::scheduler::{BackendRegistry, Scheduler, SchedulerConfig};
use maestro_core::state::{list_runs, CheckpointStatus, RunCheckpoint};
use maestro_runtime::{ExecLimits, Runtime, ScriptError};
use maestro_storage::{open_db, EventWriter};
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
    backend: Arc<dyn maestro_core::contract::backend::AgentBackend>,
    planner_cfg: maestro_planner::PlannerConfig,
) -> Result<String> {
    match source {
        ScriptSource::Nl(nl) => {
            let planned = maestro_planner::plan_workflow(nl, backend, &planner_cfg).await?;
            Ok(planned.script)
        }
        ScriptSource::Workflow(path) => std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read workflow file: {}", e)),
        ScriptSource::Script(s) => Ok(s.to_string()),
    }
}

/// The result of script resolution: script text + extracted meta.
#[derive(Debug, Clone)]
pub struct ResolvedScript {
    pub script: String,
    pub meta: Option<maestro_planner::PlanMeta>,
}

/// Resolve a script source AND extract any `meta = {...}` table.
pub async fn resolve_script_with_meta(
    source: ScriptSource<'_>,
    backend: Arc<dyn AgentBackend>,
    planner_cfg: maestro_planner::PlannerConfig,
) -> Result<ResolvedScript> {
    let script = resolve_script(source, backend, planner_cfg).await?;
    let meta = match maestro_planner::extract_meta(&script) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "meta extraction failed; continuing without meta");
            None
        }
    };
    Ok(ResolvedScript { script, meta })
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
                    CheckpointStatus::Completed
                        | CheckpointStatus::Cancelled
                        | CheckpointStatus::Failed
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
    /// Declarative workflow metadata extracted from the script's `meta = {...}` table.
    pub workflow_meta: Option<maestro_planner::PlanMeta>,
}

/// Resolve a fresh run from a script source: plans NL / reads a workflow file /
/// passes a script through, generates a new run id, and derives a task label.
/// Callers may override `task_label` / `extra_args` on the returned spec.
pub async fn resolve_fresh(
    source: ScriptSource<'_>,
    backend: Arc<dyn AgentBackend>,
    planner_cfg: maestro_planner::PlannerConfig,
) -> Result<RunSpec> {
    let task_label = match source {
        ScriptSource::Nl(nl) => nl.to_string(),
        ScriptSource::Workflow(p) => p.display().to_string(),
        ScriptSource::Script(_) => "maestro workflow".to_string(),
    };
    let resolved = resolve_script_with_meta(source, backend, planner_cfg).await?;
    let run_id = RunId::now_v7();
    Ok(RunSpec {
        run_id,
        script: resolved.script,
        task_label,
        resuming: false,
        extra_args: serde_json::json!({}),
        run_dir_name: String::new(),
        workflow_meta: resolved.meta,
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
        anyhow::bail!(
            "run {} is not resumable (status: {:?})",
            run_dir_name,
            checkpoint.status
        );
    }
    let script = std::fs::read_to_string(run_dir.join("workflow.lua")).map_err(|_| {
        anyhow::anyhow!(
            "workflow.lua not found in run directory {}",
            run_dir.display()
        )
    })?;
    Ok(RunSpec {
        run_id: checkpoint.run_id,
        run_dir_name: run_dir_name.to_string(),
        script,
        task_label: checkpoint.task,
        resuming: true,
        extra_args: serde_json::json!({}),
        workflow_meta: checkpoint.workflow_meta
            .and_then(|v| serde_json::from_value(v).ok()),
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

pub struct PreviousRun {
    pub run_dir_name: String,
    pub checkpoint: RunCheckpoint,
}

pub fn find_resumable_by_task(task: &str, base_dir: &Path) -> Result<Option<PreviousRun>> {
    let run_dirs = list_runs(base_dir)?;
    for dir in run_dirs.iter().rev() {
        let cp_path = base_dir.join(dir).join("checkpoint.json");
        let content = match std::fs::read_to_string(&cp_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let cp: RunCheckpoint = match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if cp.task == task && matches!(cp.status, CheckpointStatus::Running) {
            return Ok(Some(PreviousRun {
                run_dir_name: dir.clone(),
                checkpoint: cp,
            }));
        }
    }
    Ok(None)
}

pub fn format_duration_ago(ts: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let secs = now.saturating_sub(ts);
    if secs < 60 {
        return "just now".to_string();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m ago", mins);
    }
    let hrs = mins / 60;
    if hrs < 24 {
        return format!("{}h ago", hrs);
    }
    let days = hrs / 24;
    format!("{}d ago", days)
}

use std::time::{SystemTime, UNIX_EPOCH};

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
pub async fn prepare(
    spec: &RunSpec,
    backend: Arc<dyn AgentBackend>,
    base_dir: &Path,
    run_ctx: &RunContext,
    max_concurrency: Option<usize>,
) -> Result<PreparedRun> {
    let run_dir = base_dir.join(&spec.run_dir_name);

    // Journal: fresh runs init + persist the script (so they can be resumed);
    // resume runs open the journal to replay cached agents.
    let journal = Arc::new(
        JournalStore::new(&run_dir)
            .map_err(|e| anyhow::anyhow!("failed to open journal: {}", e))?,
    );
    if spec.resuming {
        journal
            .open(spec.run_id)
            .map_err(|e| anyhow::anyhow!("failed to open journal for resume: {}", e))?;
    } else {
        std::fs::write(run_dir.join("workflow.lua"), &spec.script)?;
        match &spec.workflow_meta {
            Some(meta) => journal
                .init_run_with_meta(spec.run_id, &spec.task_label, serde_json::to_value(meta).unwrap())
                .map_err(|e| anyhow::anyhow!("failed to init journal with meta: {}", e))?,
            None => journal
                .init_run(spec.run_id, &spec.task_label)
                .map_err(|e| anyhow::anyhow!("failed to init journal: {}", e))?,
        }
    }

    // SQLite structured persistence — shared across runs. Optional; if the DB
    // can't be opened (e.g. read-only filesystem) we keep going so journal +
    // JSONL remain the source of truth for resume.
    let sqlite_writer = match open_db(
        &run_dir
            .parent()
            .unwrap_or(base_dir)
            .join(maestro_storage::DEFAULT_DB_PATH),
    )
    .await
    {
        Ok(pool) => Some(EventWriter::new(pool)),
        Err(e) => {
            tracing::warn!(error = %e, "sqlite persistence disabled for this run");
            None
        }
    };

    // Scheduler. Journaling is handled inside the runtime (cache-key aware), so
    // no scheduler-level callback is required.
    let registry = BackendRegistry::new().with(backend);
    let default_cfg = SchedulerConfig::default();
    let actual_concurrency = max_concurrency.unwrap_or(default_cfg.max_concurrency);
    if actual_concurrency != default_cfg.max_concurrency {
        tracing::info!(
            concurrency = actual_concurrency,
            default = default_cfg.max_concurrency,
            "using custom concurrency limit"
        );
    }
    let scheduler = Scheduler::new(
        SchedulerConfig {
            max_concurrency: actual_concurrency,
            ..default_cfg
        },
        registry,
        None,
    );
    scheduler.init_run_with(spec.run_id, run_ctx.events.clone());

    // Forward the scheduler event stream into BOTH:
    //   1. Journal (checkpoint.json + events.jsonl) — for resume
    //   2. SQLite (turns/agents/runs/spans/events tables) — for UI query
    let store = journal.store();
    let mut rx = run_ctx.events.subscribe();
    let _ = run_ctx.events.send(AgentEvent::RunStarted {
        run_id: spec.run_id,
        task: spec.task_label.clone(),
        ts: chrono::Utc::now(),
    });
    let fwd_run_id = spec.run_id;
    let sqlite_writer_fwd = sqlite_writer.clone();
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
                    if let Some(w) = &sqlite_writer_fwd {
                        if let Err(e) = w.write_event(&evt).await {
                            tracing::warn!(run_id = %fwd_run_id, error = %e, "failed to persist event to sqlite");
                        }
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

    // Inject completed phase spans for resume so scripts can skip finished units.
    if spec.resuming {
        let cp_path = run_dir.join("checkpoint.json");
        if let Ok(content) = std::fs::read_to_string(&cp_path) {
            if let Ok(cp) = serde_json::from_str::<RunCheckpoint>(&content) {
                let names: Vec<String> =
                    cp.completed_spans.iter().map(|s| s.name.clone()).collect();
                if !names.is_empty() {
                    runtime.set_completed_spans(&names)?;
                }
            }
        }
    }

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
    let result = match tokio::task::spawn_blocking(move || runtime.execute(&script)).await {
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
                ts: chrono::Utc::now(),
            });
            return Err(anyhow::anyhow!("execution task panicked: {}", e));
        }
    };

    let status = if result.is_ok() {
        RunStatus::Completed
    } else {
        RunStatus::Failed
    };
    if let Err(ref e) = result {
        tracing::warn!(%run_id, error = %e, "run finished with a script error");
    }
    let report = result
        .as_ref()
        .ok()
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let _ = run_ctx.events.send(AgentEvent::RunDone {
        run_id,
        status,
        total_tokens: TokenUsage::default(),
        report,
        ts: chrono::Utc::now(),
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_core::scheduler::{BackendRegistry, Scheduler, SchedulerConfig};
    use maestro_core::{MockBackend, MockBehavior};
    use maestro_runtime::{ExecLimits, Runtime};
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    // =========================================================================
    // validate_source
    // =========================================================================

    #[test]
    fn validate_source_none() {
        let input = RunInput {
            nl: None,
            workflow: None,
            script: None,
        };
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
        let input = RunInput {
            nl: Some("hi".into()),
            workflow: None,
            script: None,
        };
        assert!(validate_source(&input).is_ok());
    }

    #[test]
    fn validate_source_workflow_only() {
        let input = RunInput {
            nl: None,
            workflow: Some(PathBuf::from("wf.lua")),
            script: None,
        };
        assert!(validate_source(&input).is_ok());
    }

    #[test]
    fn validate_source_script_only() {
        let input = RunInput {
            nl: None,
            workflow: None,
            script: Some("print(1)".into()),
        };
        assert!(validate_source(&input).is_ok());
    }

    // =========================================================================
    // slug_sources (private, tested through its 3 branches)
    // =========================================================================

    #[test]
    fn slug_sources_workflow_path() {
        let spec = RunSpec {
            run_id: RunId::now_v7(),
            run_dir_name: String::new(),
            script: String::new(),
            task_label: "scripts/clean.lua".to_string(),
            resuming: false,
            extra_args: serde_json::json!({}),
            workflow_meta: None,
        };
        let (wf, nl) = slug_sources(&spec);
        assert!(wf.is_some(), "expected workflow path");
        assert_eq!(wf.unwrap(), Path::new("scripts/clean.lua"));
        assert!(nl.is_none());
    }

    #[test]
    fn slug_sources_maestro_workflow() {
        let spec = RunSpec {
            run_id: RunId::now_v7(),
            run_dir_name: String::new(),
            script: String::new(),
            task_label: "maestro workflow".to_string(),
            resuming: false,
            extra_args: serde_json::json!({}),
            workflow_meta: None,
        };
        let (wf, nl) = slug_sources(&spec);
        assert!(wf.is_none());
        assert!(nl.is_none());
    }

    #[test]
    fn slug_sources_nl_label() {
        let spec = RunSpec {
            run_id: RunId::now_v7(),
            run_dir_name: String::new(),
            script: String::new(),
            task_label: "research AI trends".to_string(),
            resuming: false,
            extra_args: serde_json::json!({}),
            workflow_meta: None,
        };
        let (wf, nl) = slug_sources(&spec);
        assert!(wf.is_none());
        assert_eq!(nl, Some("research AI trends"));
    }

    // =========================================================================
    // assign_dir_name
    // =========================================================================

    #[test]
    fn assign_dir_name_nl_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = RunSpec {
            run_id: RunId::now_v7(),
            run_dir_name: String::new(),
            script: String::new(),
            task_label: "my test task".to_string(),
            resuming: false,
            extra_args: serde_json::json!({}),
            workflow_meta: None,
        };
        assign_dir_name(&mut spec, dir.path());
        assert!(!spec.run_dir_name.is_empty());
        assert!(
            spec.run_dir_name.starts_with("my-test-task_"),
            "expected slug prefix 'my-test-task_', got '{}'",
            spec.run_dir_name
        );
    }

    #[test]
    fn assign_dir_name_workflow_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = RunSpec {
            run_id: RunId::now_v7(),
            run_dir_name: String::new(),
            script: String::new(),
            task_label: "scripts/deep-research.lua".to_string(),
            resuming: false,
            extra_args: serde_json::json!({}),
            workflow_meta: None,
        };
        assign_dir_name(&mut spec, dir.path());
        assert!(!spec.run_dir_name.is_empty());
        assert!(
            spec.run_dir_name.starts_with("deep-research_"),
            "expected slug prefix 'deep-research_', got '{}'",
            spec.run_dir_name
        );
    }

    #[test]
    fn assign_dir_name_maestro_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = RunSpec {
            run_id: RunId::now_v7(),
            run_dir_name: String::new(),
            script: String::new(),
            task_label: "maestro workflow".to_string(),
            resuming: false,
            extra_args: serde_json::json!({}),
            workflow_meta: None,
        };
        assign_dir_name(&mut spec, dir.path());
        assert!(!spec.run_dir_name.is_empty());
        assert!(
            spec.run_dir_name.starts_with("maestro-workflow_"),
            "expected slug prefix 'maestro-workflow_', got '{}'",
            spec.run_dir_name
        );
    }

    #[test]
    fn assign_dir_name_v4_uuid_fallback_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = RunSpec {
            run_id: uuid::Uuid::new_v4(),
            run_dir_name: String::new(),
            script: String::new(),
            task_label: "test".to_string(),
            resuming: false,
            extra_args: serde_json::json!({}),
            workflow_meta: None,
        };
        assign_dir_name(&mut spec, dir.path());
        assert!(!spec.run_dir_name.is_empty());
        assert!(
            spec.run_dir_name.starts_with("test_"),
            "expected slug prefix 'test_', got '{}'",
            spec.run_dir_name
        );
        // ensure_unique may append _2, _3 etc but that's fine
    }

    // =========================================================================
    // resolve_script
    // =========================================================================

    #[tokio::test]
    async fn resolve_script_script_variant() {
        let backend: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!(""),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        ));
        let result = resolve_script(
            ScriptSource::Script("print(1)"),
            backend,
            maestro_planner::PlannerConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(result, "print(1)");
    }

    #[tokio::test]
    async fn resolve_script_workflow_variant() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.lua");
        std::fs::write(&path, "print('hello')").unwrap();

        let backend: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!(""),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        ));
        let result = resolve_script(
            ScriptSource::Workflow(&path),
            backend,
            maestro_planner::PlannerConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(result, "print('hello')");
    }

    #[tokio::test]
    async fn resolve_script_nl_variant() {
        let output =
            serde_json::json!("```lua\nmeta = { reasoning = \"test\", phases = {} }\nfunction main()\nagent({prompt='test'})\nreport({ok=true})\nend\n```");
        let backend: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output,
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        ));
        let result = resolve_script(
            ScriptSource::Nl("do something"),
            backend,
            maestro_planner::PlannerConfig::default(),
        )
        .await
        .unwrap();
        assert!(
            result.contains("report("),
            "planned script must contain report()"
        );
        assert!(!result.contains("```"), "fences should be stripped");
    }

    // =========================================================================
    // resolve_fresh
    // =========================================================================

    #[tokio::test]
    async fn resolve_fresh_script_variant() {
        let backend: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!(""),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        ));
        let spec = resolve_fresh(
            ScriptSource::Script("print(1)"),
            backend,
            maestro_planner::PlannerConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(spec.script, "print(1)");
        assert_eq!(spec.task_label, "maestro workflow");
        assert!(!spec.resuming);
    }

    #[tokio::test]
    async fn resolve_fresh_workflow_variant() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("my_workflow.lua");
        std::fs::write(&path, "report({ok=true})").unwrap();

        let backend: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!(""),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        ));
        let spec = resolve_fresh(
            ScriptSource::Workflow(&path),
            backend,
            maestro_planner::PlannerConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(spec.script, "report({ok=true})");
        assert!(spec.task_label.contains("my_workflow.lua"));
        assert!(!spec.resuming);
    }

    #[tokio::test]
    async fn resolve_fresh_nl_variant() {
        let output =
            serde_json::json!("```lua\nmeta = { reasoning = \"test\", phases = {} }\nfunction main()\nagent({prompt='test'})\nreport({ok=true})\nend\n```");
        let backend: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output,
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        ));
        let spec = resolve_fresh(
            ScriptSource::Nl("build a calculator"),
            backend,
            maestro_planner::PlannerConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(spec.task_label, "build a calculator");
        assert!(!spec.resuming);
    }

    // =========================================================================
    // check_resumable — edge cases for the remaining statuses & corrupt data
    // =========================================================================

    fn write_checkpoint(dir: &std::path::Path, status: CheckpointStatus) {
        let cp = RunCheckpoint {
            run_id: RunId::now_v7(),
            task: "t".into(),
            status,
            current_phase: 1,
            completed_phases: vec![],
            agent_results: HashMap::new(),
            findings: vec![],
            total_tokens: 0,
            created_at: 0,
            updated_at: 0,
            completed_spans: vec![],
            workflow_meta: None,
            started_agent_ids: vec![],
        };
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_string(&cp).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn resume_check_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("test_123");
        std::fs::create_dir_all(&run_dir).unwrap();
        write_checkpoint(&run_dir, CheckpointStatus::Cancelled);

        let result = check_resumable("test_123", dir.path());
        assert!(matches!(
            result,
            ResumeCheck::NotResumable(CheckpointStatus::Cancelled)
        ));
    }

    #[test]
    fn resume_check_failed() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("test_123");
        std::fs::create_dir_all(&run_dir).unwrap();
        write_checkpoint(&run_dir, CheckpointStatus::Failed);

        let result = check_resumable("test_123", dir.path());
        assert!(matches!(
            result,
            ResumeCheck::NotResumable(CheckpointStatus::Failed)
        ));
    }

    #[test]
    fn resume_check_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("test_123");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("checkpoint.json"), b"not valid json").unwrap();

        let result = check_resumable("test_123", dir.path());
        assert!(matches!(result, ResumeCheck::CanResume));
    }

    #[test]
    fn resume_check_no_status_field() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("test_123");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("checkpoint.json"),
            br#"{"run_id":"00000000-0000-0000-0000-000000000000","task":"t"}"#,
        )
        .unwrap();

        let result = check_resumable("test_123", dir.path());
        assert!(matches!(result, ResumeCheck::CanResume));
    }

    #[test]
    fn resume_check_wrong_status_type() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("test_123");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("checkpoint.json"),
            br#"{"status":123,"run_id":"00000000-0000-0000-0000-000000000000","task":"t"}"#,
        )
        .unwrap();

        let result = check_resumable("test_123", dir.path());
        assert!(matches!(result, ResumeCheck::CanResume));
    }

    #[test]
    fn resume_check_not_found() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_notfound");
        let result = check_resumable("nonexistent_123", &temp_dir);
        assert!(matches!(result, ResumeCheck::NotFound));
    }

    #[test]
    fn resume_check_completed() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("test_123");
        std::fs::create_dir_all(&run_dir).unwrap();
        write_checkpoint(&run_dir, CheckpointStatus::Completed);

        let result = check_resumable("test_123", dir.path());
        assert!(matches!(
            result,
            ResumeCheck::NotResumable(CheckpointStatus::Completed)
        ));
    }

    #[test]
    fn resume_check_running() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("test_123");
        std::fs::create_dir_all(&run_dir).unwrap();
        write_checkpoint(&run_dir, CheckpointStatus::Running);

        let result = check_resumable("test_123", dir.path());
        assert!(matches!(result, ResumeCheck::CanResume));
    }

    #[test]
    fn resume_check_no_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("test_123");
        std::fs::create_dir_all(&run_dir).unwrap();

        let result = check_resumable("test_123", dir.path());
        assert!(matches!(result, ResumeCheck::CanResume));
    }

    // =========================================================================
    // resolve_resume
    // =========================================================================

    fn make_checkpoint_json(dir: &std::path::Path, run_id: RunId, status: &str, task: &str) {
        let cp = serde_json::json!({
            "run_id": run_id,
            "task": task,
            "status": status,
            "current_phase": 0,
            "completed_phases": [],
            "agent_results": {},
            "findings": [],
            "total_tokens": 0,
            "created_at": 0,
            "updated_at": 0,
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_string(&cp).unwrap(),
        )
        .unwrap();
    }

    fn write_workflow_lua(dir: &std::path::Path, content: &str) {
        std::fs::write(dir.join("workflow.lua"), content).unwrap();
    }

    #[test]
    fn resolve_resume_success() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("my_run_123");
        std::fs::create_dir_all(&run_dir).unwrap();
        let run_id = RunId::now_v7();
        make_checkpoint_json(&run_dir, run_id, "running", "test task");
        write_workflow_lua(&run_dir, "print('resume')");

        let spec = resolve_resume("my_run_123", dir.path()).unwrap();
        assert_eq!(spec.run_id, run_id);
        assert_eq!(spec.task_label, "test task");
        assert_eq!(spec.script, "print('resume')");
        assert!(spec.resuming);
        assert_eq!(spec.run_dir_name, "my_run_123");
    }

    #[test]
    fn resolve_resume_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = format!(
            "{}",
            resolve_resume("nonexistent", dir.path()).err().unwrap()
        );
        assert!(
            err.contains("not found"),
            "expected 'not found' error, got: {}",
            err
        );
    }

    #[test]
    fn resolve_resume_not_resumable_completed() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("my_run");
        std::fs::create_dir_all(&run_dir).unwrap();
        make_checkpoint_json(&run_dir, RunId::now_v7(), "completed", "t");
        write_workflow_lua(&run_dir, "x");

        let err = format!("{}", resolve_resume("my_run", dir.path()).err().unwrap());
        assert!(
            err.contains("not resumable"),
            "expected 'not resumable' error, got: {}",
            err
        );
    }

    #[test]
    fn resolve_resume_not_resumable_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("my_run");
        std::fs::create_dir_all(&run_dir).unwrap();
        make_checkpoint_json(&run_dir, RunId::now_v7(), "cancelled", "t");
        write_workflow_lua(&run_dir, "x");

        let err = format!("{}", resolve_resume("my_run", dir.path()).err().unwrap());
        assert!(
            err.contains("not resumable"),
            "expected 'not resumable' error, got: {}",
            err
        );
    }

    #[test]
    fn resolve_resume_not_resumable_failed() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("my_run");
        std::fs::create_dir_all(&run_dir).unwrap();
        make_checkpoint_json(&run_dir, RunId::now_v7(), "failed", "t");
        write_workflow_lua(&run_dir, "x");

        let err = format!("{}", resolve_resume("my_run", dir.path()).err().unwrap());
        assert!(
            err.contains("not resumable"),
            "expected 'not resumable' error, got: {}",
            err
        );
    }

    #[test]
    fn resolve_resume_missing_workflow_lua() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("my_run");
        std::fs::create_dir_all(&run_dir).unwrap();
        make_checkpoint_json(&run_dir, RunId::now_v7(), "running", "t");
        // Intentionally do NOT write workflow.lua

        let err = format!("{}", resolve_resume("my_run", dir.path()).err().unwrap());
        assert!(
            err.contains("workflow.lua"),
            "expected 'workflow.lua' error, got: {}",
            err
        );
    }

    // =========================================================================
    // latest_resumable
    // =========================================================================

    #[test]
    fn latest_resumable_found() {
        let dir = tempfile::tempdir().unwrap();

        // Create two run dirs; the last in sorted order should be returned.
        for name in ["alpha_100", "beta_200"] {
            let d = dir.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            make_checkpoint_json(&d, RunId::now_v7(), "running", "t");
        }

        let found = latest_resumable(dir.path()).unwrap();
        assert_eq!(found, "beta_200");
    }

    #[test]
    fn latest_resumable_not_found() {
        let dir = tempfile::tempdir().unwrap();
        // Empty directory — no runs at all.
        let err = latest_resumable(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("no resumable run"),
            "expected 'no resumable run' error, got: {}",
            err
        );
    }

    // =========================================================================
    // prepare — fresh run
    // =========================================================================

    fn make_prepare_backend() -> Arc<dyn AgentBackend> {
        Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        ))
    }

    #[tokio::test]
    async fn prepare_fresh_run() {
        let dir = tempfile::tempdir().unwrap();
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let run_dir_name = "fresh_run_42".to_string();

        // Create the run directory beforehand (prepare creates JournalStore there)
        let run_dir = dir.path().join(&run_dir_name);
        std::fs::create_dir_all(&run_dir).unwrap();

        let spec = RunSpec {
            run_id,
            run_dir_name,
            script: "report({ok=true})".to_string(),
            task_label: "fresh test".to_string(),
            resuming: false,
            extra_args: serde_json::json!({}),
            workflow_meta: None,
        };

        let backend = make_prepare_backend();
        let _prepared = prepare(&spec, backend, dir.path(), &run_ctx, None)
            .await
            .unwrap();

        // Fresh run: workflow.lua should have been written
        assert!(
            run_dir.join("workflow.lua").exists(),
            "workflow.lua should exist"
        );
        assert!(
            run_dir.join("checkpoint.json").exists(),
            "checkpoint.json should exist"
        );

        // Journal and runtime should be accessible
        let content = std::fs::read_to_string(run_dir.join("workflow.lua")).unwrap();
        assert_eq!(content, "report({ok=true})");
    }

    // =========================================================================
    // prepare — resume run
    // =========================================================================

    #[tokio::test]
    async fn prepare_resume_run() {
        let dir = tempfile::tempdir().unwrap();
        let run_id = RunId::now_v7();
        let run_dir_name = "resume_run_99".to_string();
        let run_dir = dir.path().join(&run_dir_name);
        std::fs::create_dir_all(&run_dir).unwrap();

        // Seed a journal + workflow.lua as if a prior run had been started.
        let journal = JournalStore::new(&run_dir).unwrap();
        journal.init_run(run_id, "resume test").unwrap();
        write_workflow_lua(&run_dir, "report({resumed=true})");
        drop(journal);

        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };

        let spec = RunSpec {
            run_id,
            run_dir_name,
            script: String::new(),
            task_label: String::new(),
            resuming: true,
            extra_args: serde_json::json!({}),
            workflow_meta: None,
        };

        let backend = make_prepare_backend();
        let _prepared = prepare(&spec, backend, dir.path(), &run_ctx, None)
            .await
            .unwrap();

        // Resume should NOT overwrite workflow.lua
        let content = std::fs::read_to_string(run_dir.join("workflow.lua")).unwrap();
        assert_eq!(content, "report({resumed=true})");
    }

    // =========================================================================
    // execute
    // =========================================================================

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn execute_script_success() {
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(256);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };

        let backend = make_prepare_backend();
        let registry = BackendRegistry::new().with(backend);
        let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
        scheduler.init_run_with(run_id, run_ctx.events.clone());

        let handle = tokio::runtime::Handle::current();
        let runtime = Runtime::new(
            scheduler,
            run_ctx.clone(),
            serde_json::json!({}),
            ExecLimits::default(),
            None,
            handle,
        )
        .unwrap();

        let script = "meta = { reasoning = \"test\", phases = {{ label = \"work\" }} }\nfunction main() report({hello = 'world'}) end".to_string();
        let result = execute(&run_ctx, runtime, script).await.unwrap();
        assert!(result.is_ok());
        let val = result.unwrap();
        assert_eq!(val["hello"], "world");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn execute_script_error() {
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(256);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };

        let backend = make_prepare_backend();
        let registry = BackendRegistry::new().with(backend);
        let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
        scheduler.init_run_with(run_id, run_ctx.events.clone());

        let handle = tokio::runtime::Handle::current();
        let runtime = Runtime::new(
            scheduler,
            run_ctx.clone(),
            serde_json::json!({}),
            ExecLimits::default(),
            None,
            handle,
        )
        .unwrap();

        // Invalid Lua syntax should produce a ScriptError
        let script = "this is not valid lua @@".to_string();
        let result = execute(&run_ctx, runtime, script).await.unwrap();
        assert!(result.is_err());
        match result {
            Err(ScriptError::Syntax(_)) => {} // expected
            _ => panic!("expected Syntax error, got {:?}", result),
        }
    }

    // =========================================================================
    // find_resumable_by_task
    // =========================================================================

    fn make_checkpoint_with_task(dir: &std::path::Path, task: &str, status: &str) {
        let cp = serde_json::json!({
            "run_id": RunId::now_v7(),
            "task": task,
            "status": status,
            "current_phase": 0,
            "completed_phases": [],
            "agent_results": {},
            "findings": [],
            "total_tokens": 0,
            "created_at": 0,
            "updated_at": 0,
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_string(&cp).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn find_resumable_by_task_match() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("clean_100");
        std::fs::create_dir_all(&run_dir).unwrap();
        make_checkpoint_with_task(&run_dir, "scripts/clean.lua", "running");

        let result = find_resumable_by_task("scripts/clean.lua", dir.path()).unwrap();
        assert!(result.is_some());
        let prev = result.unwrap();
        assert_eq!(prev.run_dir_name, "clean_100");
        assert_eq!(prev.checkpoint.task, "scripts/clean.lua");
    }

    #[test]
    fn find_resumable_by_task_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("clean_100");
        std::fs::create_dir_all(&run_dir).unwrap();
        make_checkpoint_with_task(&run_dir, "scripts/clean.lua", "running");

        let result = find_resumable_by_task("scripts/other.lua", dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn find_resumable_by_task_skips_completed() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("clean_100");
        std::fs::create_dir_all(&run_dir).unwrap();
        make_checkpoint_with_task(&run_dir, "scripts/clean.lua", "completed");

        let result = find_resumable_by_task("scripts/clean.lua", dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn find_resumable_by_task_returns_latest() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["clean_100", "clean_200"] {
            let run_dir = dir.path().join(name);
            std::fs::create_dir_all(&run_dir).unwrap();
            make_checkpoint_with_task(&run_dir, "scripts/clean.lua", "running");
        }

        let result = find_resumable_by_task("scripts/clean.lua", dir.path()).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().run_dir_name, "clean_200");
    }

    #[test]
    fn find_resumable_by_task_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_resumable_by_task("scripts/clean.lua", dir.path()).unwrap();
        assert!(result.is_none());
    }

    // =========================================================================
    // format_duration_ago
    // =========================================================================

    #[test]
    fn format_duration_ago_just_now() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(format_duration_ago(now), "just now");
        assert_eq!(format_duration_ago(now - 30), "just now");
    }

    #[test]
    fn format_duration_ago_minutes() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(format_duration_ago(now - 120), "2m ago");
        assert_eq!(format_duration_ago(now - 3540), "59m ago");
    }

    #[test]
    fn format_duration_ago_hours() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(format_duration_ago(now - 3600), "1h ago");
        assert_eq!(format_duration_ago(now - 7200), "2h ago");
    }

    #[test]
    fn format_duration_ago_days() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(format_duration_ago(now - 86400), "1d ago");
        assert_eq!(format_duration_ago(now - 172800), "2d ago");
    }
}
