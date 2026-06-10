//! `run` subcommand: detect the backend, resolve (and optionally confirm) the
//! script via the shared run service ([`maestro::service::run`]), then drive the
//! TUI / headless presentation. This is the sole presentation layer for `run`;
//! the library exposes only the presentation-free service.

use crate::backend;
use crate::commands::runs_base_dir;
use crate::RunArgs;
use anyhow::Result;
use futures::FutureExt;
use maestro::core::contract::backend::RunContext;
use maestro::runtime::Runtime;
use maestro::service::run as svc;
use std::path::{Path, PathBuf};

pub async fn run_workflow(args: RunArgs) -> Result<()> {
    let is_nl = args.nl.is_some();
    let backend_id = match args.backend.as_deref() {
        Some(id) => id.to_string(),
        None => {
            let detected = backend::detect_backend();
            if is_nl && detected == "mock" {
                anyhow::bail!(
                    "NL mode requires a real LLM backend. \
                     Install opencode (https://opencode.ai) or specify --backend <id>"
                );
            }
            if is_nl {
                eprintln!("ℹ  no --backend specified, auto-detected: {}", detected);
            }
            detected.to_string()
        }
    };
    let backend = backend::create_backend(&backend_id)?;
    let base_dir = runs_base_dir();

    // Resolve a fully-specified run (script + run id + resume flag). NL planning,
    // workflow-file reads and script pass-through all live in the service; this
    // resolves exactly once (so `--confirm` shows the same script that runs).
    let mut spec = if args.resume {
        let run_id = svc::latest_resumable(&base_dir)?;
        svc::resolve_resume(run_id, &base_dir)?
    } else {
        let source = if let Some(nl) = args.nl.as_deref() {
            svc::ScriptSource::Nl(nl)
        } else if let Some(wf) = args.workflow.as_deref() {
            if !wf.exists() {
                anyhow::bail!("workflow file not found: {}", wf.display());
            }
            svc::ScriptSource::Workflow(wf)
        } else {
            anyhow::bail!("either a natural language prompt or --workflow <file> is required");
        };
        svc::resolve_fresh(source, backend.clone()).await?
    };
    // `--args` takes precedence over the positional `extra_args`. A malformed
    // JSON value is a hard error rather than being silently dropped.
    if let Some(s) = args.args_json.as_ref().or(args.extra_args.as_ref()) {
        spec.extra_args = serde_json::from_str(s)
            .map_err(|e| anyhow::anyhow!("invalid workflow args JSON: {}", e))?;
    }

    if args.confirm {
        println!("=== Workflow Script ===");
        println!("{}", spec.script);
        println!("=======================");

        print!("Approve execution? [y/N] ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // The CLI owns the event channel + cancel token (subscribed locally for
    // TUI/headless output).
    let (events_tx, _events_rx) = tokio::sync::broadcast::channel(256);
    let run_ctx = RunContext {
        run_id: spec.run_id,
        cancel: tokio_util::sync::CancellationToken::new(),
        events: events_tx,
    };

    let prepared = svc::prepare(&spec, backend, &base_dir, &run_ctx)?;
    if spec.resuming {
        println!(
            "Resuming run {} ({} agents cached)",
            spec.run_id,
            prepared.journal.completed_keys().len()
        );
    }

    if args.headless {
        run_headless(run_ctx, prepared.runtime, spec.script, args.output).await
    } else {
        run_tui(run_ctx, prepared.runtime, spec.script, args.output).await
    }
}

/// Persist the final report value to `path`.
///
/// Convention: if the report is a string it is written verbatim; if it is an
/// object with a string `markdown` field, that field is written verbatim (so a
/// research workflow can emit a clean `.md`); otherwise the report is written as
/// pretty JSON.
fn write_report(path: &Path, report: &serde_json::Value) -> Result<()> {
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
fn print_progress(evt: &maestro::core::contract::event::AgentEvent) {
    use maestro::core::contract::event::{AgentEvent, LogLevel};
    match evt {
        AgentEvent::PhaseStarted { phase_id, label, planned, .. } => {
            eprintln!("▶ phase {} · {} ({} planned)", phase_id, label, planned);
        }
        AgentEvent::AgentStarted { prompt_preview, .. } => {
            let preview: String = prompt_preview.chars().take(72).collect();
            eprintln!("  ↳ agent: {}…", preview);
        }
        AgentEvent::AgentDone { status, elapsed_ms, .. } => {
            eprintln!("  ✓ agent {:?} ({} ms)", status, elapsed_ms);
        }
        AgentEvent::Log { level, msg, .. } => {
            let mark = match level {
                LogLevel::Warn => "⚠",
                LogLevel::Error => "✗",
                _ => "·",
            };
            eprintln!("  {} {}", mark, msg);
        }
        _ => {}
    }
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

    let result = svc::execute(&run_ctx, rt, script).await?;

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

    let result = svc::execute(&run_ctx, rt, script).await?;
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
