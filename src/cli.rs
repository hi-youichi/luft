//! `cli` â€” CLI + TUI + headless (M7/M8). See code-design Â§8.
//!
//! Commands:
//! - `maestro run "<NL>"` â€” NL â†’ Lua via planner, then execute
//! - `maestro run /<wf>` â€” Run workflow.lua file
//! - `maestro run --resume` â€” Resume from checkpoint
//! - `maestro run --headless` â€” JSONL output mode
//! - `maestro run --approve` â€” Auto-approve without prompt
//! - `maestro status <run_id>` â€” Check run status
//! - `maestro logs <run_id>` â€” View run logs
//! - `maestro list` â€” List all runs

use crate::core::contract::backend::RunContext;
use crate::core::contract::event::RunStatus;
use crate::core::contract::ids::TokenUsage;
use crate::core::journal::JournalStore;
use crate::core::scheduler::{BackendRegistry, Scheduler, SchedulerConfig};
use crate::core::state::{list_runs, RunCheckpoint, CheckpointStatus};
use crate::runtime::{ExecLimits, Runtime};
use anyhow::Result;
use futures::FutureExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Run mode: TUI (interactive) or headless (JSONL).
#[derive(Debug, Clone)]
pub enum RunMode {
    /// Interactive TUI with ratatui
    Tui,
    /// JSON Lines output to stdout
    Headless,
}

/// CLI arguments for `run` command.
#[derive(Debug, Clone)]
pub struct RunArgs {
    /// Natural language prompt (for `run "<NL>"` mode)
    pub nl: Option<String>,
    /// Path to workflow.lua file (for `run /<wf>` mode)
    pub workflow: Option<PathBuf>,
    /// Pre-generated script (e.g. from the agent-driven planner), used for fresh
    /// NL runs in lieu of a `--workflow` file.
    pub script: Option<String>,
    /// Resume from previous run
    pub resume: bool,
    /// Run mode
    pub mode: RunMode,
    /// Auto-approve (skip confirmation)
    pub approve: bool,
    /// Extra args passed to workflow
    pub extra_args: serde_json::Value,
    /// Optional path to write the final report to (see [`write_report`]).
    pub output: Option<PathBuf>,
}

impl RunArgs {
    /// Create args manually (for testing / programmatic use).
    #[allow(unused)]
    pub fn new(
        nl: Option<String>,
        workflow: Option<PathBuf>,
        resume: bool,
        mode: RunMode,
        approve: bool,
    ) -> Self {
        Self {
            nl,
            workflow,
            script: None,
            resume,
            mode,
            approve,
            extra_args: serde_json::json!({}),
            output: None,
        }
    }
}

/// Status command output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StatusOutput {
    pub run_id: String,
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

impl From<&RunCheckpoint> for StatusOutput {
    fn from(cp: &RunCheckpoint) -> Self {
        let created = chrono::DateTime::from_timestamp(cp.created_at as i64, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();
        let updated = chrono::DateTime::from_timestamp(cp.updated_at as i64, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();

        Self {
            run_id: cp.run_id.to_string(),
            task: cp.task.clone(),
            status: format!("{:?}", cp.status).to_lowercase(),
            current_phase: cp.current_phase,
            completed_phases: cp.completed_phases.len(),
            total_agents: cp.agent_results.len(),
            completed_agents: cp.agent_results.values().filter(|r| r.status == "ok").count(),
            total_tokens: cp.total_tokens,
            created_at: created,
            updated_at: updated,
        }
    }
}

/// List all runs.
pub fn list_runs_cmd(base_dir: &Path) -> Result<Vec<StatusOutput>> {
    crate::service::query::list_runs(base_dir)
}

/// Get status of a specific run.
pub fn status_cmd(run_id: uuid::Uuid, base_dir: &Path) -> Result<Option<StatusOutput>> {
    crate::service::query::get_status(run_id, base_dir)
}

/// Get logs for a specific run.
pub fn logs_cmd(run_id: uuid::Uuid, base_dir: &Path, limit: Option<usize>) -> Result<Vec<String>> {
    crate::service::query::get_logs(run_id, base_dir, limit)
}

/// Get findings for a specific run.
pub fn findings_cmd(run_id: uuid::Uuid, base_dir: &Path) -> Result<Vec<crate::core::contract::finding::Finding>> {
    crate::service::query::get_findings(run_id, base_dir)
}

/// Cancel a running workflow.
pub fn cancel_cmd(run_id: uuid::Uuid, base_dir: &Path) -> Result<()> {
    crate::service::query::cancel_run(run_id, base_dir)?;
    println!("Run {} cancelled", run_id);
    Ok(())
}

/// Main entry point for `maestro run`.
pub async fn run(backend: Arc<dyn crate::core::contract::backend::AgentBackend>, args: RunArgs) -> Result<()> {
    let base_dir = std::path::PathBuf::from(".").join(".maestro").join("runs");

    // Resolve the script, run id, and whether this is a resume.
    let (script, run_id, resuming) = if args.resume {
        let run_ids = list_runs(&base_dir)?;
        let resume_run_id = run_ids
            .iter()
            .rev()
            .copied()
            .find(|&rid| base_dir.join(rid.to_string()).join("checkpoint.json").exists())
            .ok_or_else(|| anyhow::anyhow!("no resumable run found"))?;

        let run_dir = base_dir.join(resume_run_id.to_string());
        let content = std::fs::read_to_string(run_dir.join("checkpoint.json"))?;
        let checkpoint: RunCheckpoint = serde_json::from_str(&content)?;
        if matches!(
            checkpoint.status,
            CheckpointStatus::Completed | CheckpointStatus::Cancelled | CheckpointStatus::Failed
        ) {
            anyhow::bail!("run {} is not resumable (status: {:?})", resume_run_id, checkpoint.status);
        }

        let script = std::fs::read_to_string(run_dir.join("workflow.lua"))
            .map_err(|_| anyhow::anyhow!("workflow.lua not found in run directory {}", run_dir.display()))?;
        (script, resume_run_id, true)
    } else if let Some(wf) = args.workflow.clone() {
        (std::fs::read_to_string(&wf)?, uuid::Uuid::now_v7(), false)
    } else if let Some(script) = args.script.clone() {
        // Agent-driven planner output, passed through from the CLI entrypoint.
        (script, uuid::Uuid::now_v7(), false)
    } else {
        anyhow::bail!("either a workflow (--workflow / generated from NL) or --resume is required")
    };

    let run_dir = base_dir.join(run_id.to_string());

    // Journal is always on: fresh runs init + persist the script (so they can be
    // resumed later); resume runs open the journal to replay cached agents.
    let journal = Arc::new(
        JournalStore::new(&run_dir).map_err(|e| anyhow::anyhow!("failed to open journal: {}", e))?,
    );
    if resuming {
        journal
            .open(run_id)
            .map_err(|e| anyhow::anyhow!("failed to open journal for resume: {}", e))?;
        println!("Resuming run {} ({} agents cached)", run_id, journal.completed_keys().len());
    } else {
        // Prefer the real task description (NL prompt or workflow file) so the
        // checkpoint is identifiable in `list`/`status`.
        let task_label = args
            .nl
            .as_deref()
            .map(str::to_string)
            .or_else(|| args.workflow.as_ref().map(|w| w.display().to_string()))
            .unwrap_or_else(|| "maestro workflow".to_string());
        journal
            .init_run(run_id, &task_label)
            .map_err(|e| anyhow::anyhow!("failed to init journal: {}", e))?;
        std::fs::write(run_dir.join("workflow.lua"), &script)?;
    }

    // Scheduler. Journaling is handled inside the runtime (cache-key aware), so
    // no scheduler-level callback is required.
    let registry = BackendRegistry::new().with(backend.clone());
    let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);

    // Run context + event bus.
    let (events_tx, _events_rx) = tokio::sync::broadcast::channel(256);
    let run_ctx = RunContext {
        run_id,
        cancel: tokio_util::sync::CancellationToken::new(),
        events: events_tx,
    };
    scheduler.init_run_with(run_id, run_ctx.events.clone());

    // Forward the scheduler event stream into the journal's run store â€” the
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
    let rt = Runtime::new(
        scheduler.clone(),
        run_ctx.clone(),
        args.extra_args,
        ExecLimits::default(),
        Some(journal.clone()),
        handle,
    )?;

    match args.mode {
        RunMode::Tui => run_tui(run_ctx, rt, script, args.output.clone()).await?,
        RunMode::Headless => run_headless(run_ctx, rt, script, args.output.clone()).await?,
    }

    Ok(())
}

/// Persist the final report value to `path`.
///
/// Convention: if the report is a string it is written verbatim; if it is an
/// object with a string `markdown` field, that field is written verbatim (so a
/// research workflow can emit a clean `.md`); otherwise the report is written as
/// pretty JSON.
fn write_report(path: &std::path::Path, report: &serde_json::Value) -> Result<()> {
    let content = match report {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => match map.get("markdown") {
            Some(serde_json::Value::String(s)) => s.clone(),
            _ => serde_json::to_string_pretty(report)?,
        },
        _ => serde_json::to_string_pretty(report)?,
    };
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// Print a concise one-line progress marker for a live event (to stderr, so it
/// never pollutes the headless JSONL on stdout).
fn print_progress(evt: &crate::core::contract::event::AgentEvent) {
    use crate::core::contract::event::{AgentEvent, LogLevel};
    match evt {
        AgentEvent::PhaseStarted { phase_id, label, planned, .. } => {
            eprintln!("â–¶ phase {} Â· {} ({} planned)", phase_id, label, planned);
        }
        AgentEvent::AgentStarted { prompt_preview, .. } => {
            let preview: String = prompt_preview.chars().take(72).collect();
            eprintln!("  â†³ agent: {}â€¦", preview);
        }
        AgentEvent::AgentDone { status, elapsed_ms, .. } => {
            eprintln!("  âœ“ agent {:?} ({} ms)", status, elapsed_ms);
        }
        AgentEvent::Log { level, msg, .. } => {
            let mark = match level {
                LogLevel::Warn => "âš ",
                LogLevel::Error => "âœ—",
                _ => "Â·",
            };
            eprintln!("  {} {}", mark, msg);
        }
        _ => {}
    }
}

/// Execute the Lua runtime on a blocking thread and emit a terminal RunDone
/// event. Returns the report value (or the script error).
async fn execute_runtime(
    run_ctx: &RunContext,
    rt: Runtime,
    script: String,
) -> Result<std::result::Result<serde_json::Value, crate::runtime::ScriptError>> {
    use crate::core::contract::event::AgentEvent;

    let run_id = run_ctx.run_id;
    // mlua is not Send-safe to drive from an async worker thread, and the SDK
    // primitives call Handle::block_on internally â€” both require a blocking
    // thread outside the async runtime.
    let result = tokio::task::spawn_blocking(move || rt.execute(&script))
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

/// Headless mode: output events as JSONL, then the final report.
async fn run_headless(
    run_ctx: RunContext,
    rt: Runtime,
    script: String,
    output: Option<PathBuf>,
) -> Result<()> {
    use tokio::time::Duration;

    let run_id = run_ctx.run_id;
    let mut events_rx = run_ctx.events.subscribe();

    let result = execute_runtime(&run_ctx, rt, script).await?;

    // Drain events with a short grace period for the background forwarder.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    loop {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        match events_rx.recv().now_or_never() {
            Some(Ok(evt)) => println!("{}", serde_json::to_string(&evt)?),
            Some(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break,
            Some(Err(_)) => continue,
            None => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }

    match result {
        Ok(report) => {
            if let Some(path) = &output {
                write_report(path, &report)?;
                eprintln!("Report written to {}", path.display());
            }
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "type": "report",
                    "run_id": run_id.to_string(),
                    "report": report,
                }))?
            )
        }
        Err(e) => eprintln!("Execution error: {}", e),
    }
    Ok(())
}

/// TUI mode (simple text output for now): execute and print the report.
/// While the script runs, concise progress markers stream to stderr.
async fn run_tui(
    run_ctx: RunContext,
    rt: Runtime,
    script: String,
    output: Option<PathBuf>,
) -> Result<()> {
    // Live progress: print events to stderr as they arrive (stdout is reserved
    // for the final report). Aborted once execution finishes.
    let mut progress_rx = run_ctx.events.subscribe();
    let progress = tokio::spawn(async move {
        loop {
            match progress_rx.recv().await {
                Ok(evt) => print_progress(&evt),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(_) => continue,
            }
        }
    });

    let result = execute_runtime(&run_ctx, rt, script).await?;
    // Let the background forwarder flush the final events to disk.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    progress.abort();
    match result {
        Ok(report) => {
            if let Some(path) = &output {
                write_report(path, &report)?;
                eprintln!("\nReport written to {}", path.display());
            }
            println!("\n=== Report ===");
            println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
        }
        Err(e) => eprintln!("Execution error: {}", e),
    }
    Ok(())
}