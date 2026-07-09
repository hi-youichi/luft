//! `run` subcommand: detect the backend, resolve (and optionally confirm) the
//! script via the shared run service ([`maestro::service::run`]), then drive
//! output. This is the sole presentation layer for `run`;
//! the library exposes only the presentation-free service.
//!
//! # Module layout
//!
//! The `run` command is structured so each concern lives in its own small
//! helper and `run_workflow` reads top-down as a short orchestration:
//!
//! * [`resolve_backend_id`] — NL-mode validation + auto-detect logging.
//! * [`load_model_and_planner`] — config / CLI model + planner resolution.
//! * [`prompt_resume_if_present`] — interactive resume prompt for the same task.
//! * [`resolve_run_spec`] — `--resume`, prompt-override, or fresh resolution.
//! * [`prompt_confirm_script`] — `--confirm` interactive gate.
//! * [`run_auto_fix_loop`] — the retry loop.
//!
//! `run_headless` is split similarly into:
//!
//! * [`spawn_printer_task`] — drains events, renders, logs, writes artifacts.
//! * [`spawn_tick_task`] — 1-second wall-clock header timer.
//! * [`join_and_report`] — drives execution, joins both tasks, reports results.
//!
//! Both report sinks ([`write_report`], [`print_report`]) share
//! [`extract_report_text`] so a `String` or `Object{markdown: String}` report
//! is treated identically regardless of where it lands.

use crate::backend;
use crate::commands::artifact_writer::ArtifactWriter;
use crate::commands::event_log::EventLogger;
use crate::commands::phase_renderer::PhaseRenderer;
use crate::commands::runs_base_dir;
use crate::RunArgs;
use anyhow::Result;
use maestro::core::contract::backend::RunContext;
use maestro::core::contract::event::AgentEvent;
use maestro::core::contract::ids::AgentId;
use maestro::core::{AgentBackend, MockFileBackend, MockStats};
use maestro::runtime::Runtime;
use maestro::service::run as svc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// ── tuning constants ───────────────────────────────────────────────────────

/// Capacity of the per-attempt [`AgentEvent`] broadcast bus.
///
/// The runtime is event-heavy (one `AcpRaw` per ACP `session/update`), so the
/// bus needs enough headroom for the printer task to keep up without lag.
/// `2048` was chosen empirically; smaller buffers produce noisy
/// `⚠ event stream lagged` warnings on long workflows.
const EVENT_BUS_CAPACITY: usize = 2048;

/// Capacity of the auxiliary [`AgentEvent`] channel used by [`try_fix_script`]
/// to ask the LLM for a fixed script. It only carries a single run's terminal
/// events; `8` is far more than enough.
const FIX_CHANNEL_CAPACITY: usize = 8;

/// How long the printer task is allowed to drain remaining events after
/// execution completes before we abandon it. `2s` gives the high-frequency
/// `AcpRaw` tail a chance to settle without blocking the next attempt.
const PRINT_JOIN_TIMEOUT_SECS: u64 = 2;

/// How long we wait for the tick (wall-clock timer) task to exit after the
/// printer drains. Tick is sub-second; `1s` is a generous upper bound.
const TICK_JOIN_TIMEOUT_SECS: u64 = 1;

/// Wall-clock re-render interval for the live phase header timer.
const TICK_INTERVAL_SECS: u64 = 1;

/// Per-attempt timeout passed to the backend when asking the LLM to fix a
/// broken Lua script. `60s` is enough headroom for a typical model to
/// rewrite a small workflow.
const FIX_LUA_TIMEOUT_SECS: u64 = 60;

// ── public entry point ─────────────────────────────────────────────────────

/// Detect the backend, resolve (and optionally confirm) the workflow, then
/// drive headless output. Auto-fix retries are handled in
/// [`run_auto_fix_loop`].
pub async fn run_workflow(
    args: RunArgs,
    parent_cancel: CancellationToken,
    sig_tx: tokio::sync::broadcast::Sender<crate::signal::SignalInfo>,
) -> Result<()> {
    let backend_id = resolve_backend_id(&args)?;
    let (model, planner_cfg) = load_model_and_planner(&args);
    let base_dir = runs_base_dir();

    let resume_override = prompt_resume_if_present(&args, &base_dir)?;

    let mut spec = resolve_run_spec(
        &args,
        &backend_id,
        model.clone(),
        planner_cfg,
        &base_dir,
        resume_override,
    )
    .await?;

    // `--args` takes precedence over the positional `extra_args`.
    if let Some(s) = args.args_json.as_ref().or(args.extra_args.as_ref()) {
        spec.extra_args = serde_json::from_str(s)
            .map_err(|e| anyhow::anyhow!("invalid workflow args JSON: {}", e))?;
    }

    if args.confirm && !prompt_confirm_script(&spec.script)? {
        return Ok(());
    }

    let max_att = if args.auto_fix {
        args.max_fix_attempts.max(1)
    } else {
        1
    };

    run_auto_fix_loop(
        &args,
        &spec,
        &backend_id,
        model,
        &base_dir,
        max_att,
        parent_cancel,
        sig_tx,
    )
    .await
}

// ── run_workflow helpers (F1) ──────────────────────────────────────────────

/// Validate the backend id and surface user-facing errors for NL-vs-mock
/// mismatches. Logs auto-detected backends so users can audit what ran.
fn resolve_backend_id(args: &RunArgs) -> Result<String> {
    let backend_id = crate::config::resolve_default_backend(args.backend.as_deref());
    let is_nl = args.nl.is_some();
    if is_nl && backend_id == "mock" {
        anyhow::bail!(
            "NL mode requires a real LLM backend. \
             Install opencode (https://opencode.ai) or specify --backend <id>"
        );
    }
    if is_nl && backend_id == "mockfile" {
        anyhow::bail!(
            "mockfile backend requires --workflow. Use `maestro generate --with-mock` first."
        );
    }
    if is_nl && args.backend.is_none() {
        eprintln!(
            "\u{2139}  no --backend specified, auto-detected: {}",
            backend_id
        );
    }
    Ok(backend_id)
}

/// Resolve the runtime model and planner model from CLI flags + config, with
/// the planner falling back to the runtime model when no override is given.
fn load_model_and_planner(args: &RunArgs) -> (Option<String>, maestro::planner::PlannerConfig) {
    let config = crate::config::load_config();
    let model = crate::config::resolve_model(
        args.model.as_deref(),
        config.as_ref().and_then(|c| c.backend.model.as_deref()),
    );
    if let Some(ref m) = model {
        eprintln!("\u{2139}  using model: {}", m);
    }
    let planner_model = crate::config::resolve_planner_model(
        args.planner_model.as_deref(),
        config.as_ref().and_then(|c| c.planner.model.as_deref()),
        model.as_deref(),
    );
    let planner_cfg = maestro::planner::PlannerConfig {
        planner_model,
        ..Default::default()
    };
    (model, planner_cfg)
}

/// If there's a previous run for the same task and the user is at a TTY,
/// offer to resume it. Returns the chosen run dir name, or `None` when:
///   * `--resume` is already set (handled elsewhere),
///   * we're not attached to a TTY,
///   * no NL / workflow source was provided, or
///   * no previous run was found / the user declined.
fn prompt_resume_if_present(args: &RunArgs, base_dir: &Path) -> Result<Option<String>> {
    if args.resume || !console::user_attended() {
        return Ok(None);
    }
    let task = match args.nl.as_deref() {
        Some(nl) => nl.to_string(),
        None => match args.workflow.as_deref() {
            Some(wf) => wf.display().to_string(),
            None => return Ok(None),
        },
    };
    let prev = match svc::find_resumable_by_task(&task, base_dir)? {
        Some(p) => p,
        None => return Ok(None),
    };
    let agent_count = prev.checkpoint.agent_results.len();
    let ago = svc::format_duration_ago(prev.checkpoint.updated_at);
    eprint!(
        "\u{26a0} Previous run detected (\u{1f552} {}, {} agents) Resume? [Y/n] ",
        ago, agent_count
    );
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("y") {
        Ok(Some(prev.run_dir_name))
    } else {
        Ok(None)
    }
}

/// Build the [`svc::RunSpec`] for this invocation. Three cases:
///   * `--resume` → resume the most recent checkpoint,
///   * an explicit resume override → resume that one,
///   * otherwise → fresh resolve from the CLI source.
async fn resolve_run_spec(
    args: &RunArgs,
    backend_id: &str,
    model: Option<String>,
    planner_cfg: maestro::planner::PlannerConfig,
    base_dir: &Path,
    resume_override: Option<String>,
) -> Result<svc::RunSpec> {
    if args.resume {
        let run_dir = svc::latest_resumable(base_dir)?;
        return svc::resolve_resume(&run_dir, base_dir);
    }
    if let Some(run_dir) = resume_override {
        return svc::resolve_resume(&run_dir, base_dir);
    }
    let (backend, _mock_stats) = create_run_backend(
        backend_id,
        args.workflow.as_deref(),
        !args.no_acp_raw,
        model.clone(),
    )?;
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
    let mut spec = svc::resolve_fresh(source, backend, planner_cfg).await?;
    svc::assign_dir_name(&mut spec, base_dir);
    Ok(spec)
}

/// `--confirm` flow: print the planned Lua and ask the user to approve.
/// Returns `Ok(true)` when the user types `y`/`Y` (case-insensitive),
/// otherwise `Ok(false)`. The caller is responsible for exiting cleanly
/// when the answer is no.
fn prompt_confirm_script(script: &str) -> Result<bool> {
    println!("=== Workflow Script ===");
    println!("{}", script);
    println!("=======================");
    print!("Approve execution? [y/N] ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let approved = input.trim().eq_ignore_ascii_case("y");
    if !approved {
        println!("Aborted.");
    }
    Ok(approved)
}

/// Run the workflow in a retry loop, calling the LLM to fix a broken Lua
/// script on failure (when `--auto-fix` is set — caller pre-flattens that
/// into `max_att`). Each iteration gets a fresh event channel + cancel
/// token so a failed attempt can't poison the next one.
///
/// The long parameter list is deliberate: every value here is a resolved
/// dependency that the loop closes over, so introducing a config struct
/// would just shuffle bytes. Kept as one orchestrator to keep
/// `run_workflow` short.
#[allow(clippy::too_many_arguments)]
async fn run_auto_fix_loop(
    args: &RunArgs,
    spec: &svc::RunSpec,
    backend_id: &str,
    model: Option<String>,
    base_dir: &Path,
    max_att: u32,
    parent_cancel: CancellationToken,
    sig_tx: tokio::sync::broadcast::Sender<crate::signal::SignalInfo>,
) -> Result<()> {
    let mut current_script = spec.script.clone();
    for attempt in 1..=max_att {
        let cancel = parent_cancel.child_token();
        let (tx, _rx_keep) = tokio::sync::broadcast::channel(EVENT_BUS_CAPACITY);
        let ctx = RunContext {
            run_id: spec.run_id,
            cancel: cancel.clone(),
            events: tx.clone(),
        };

        // OS-signal forwarder: translate a process signal into a
        // `SignalReceived` event (lands in `events.jsonl`) and cancel the
        // attempt token. `_rx_keep` keeps the broadcast alive even before
        // `run_headless` subscribes its printer.
        let run_id_sig = spec.run_id;
        let tx_sig = tx.clone();
        let cancel_sig = cancel.clone();
        let mut sig_rx = sig_tx.subscribe();
        let fwd = tokio::spawn(async move {
            if let Ok(info) = sig_rx.recv().await {
                let _ = tx_sig.send(AgentEvent::SignalReceived {
                    run_id: Some(run_id_sig),
                    signal: info.signal,
                    ts: info.ts,
                });
                cancel_sig.cancel();
            }
        });

        let (attempt_backend, mock_stats) = create_run_backend(
            backend_id,
            args.workflow.as_deref(),
            !args.no_acp_raw,
            model.clone(),
        )?;

        let prepared =
            svc::prepare(spec, attempt_backend, base_dir, &ctx, args.max_concurrency).await?;

        if spec.resuming {
            println!(
                "Resuming run {} ({} agents cached)",
                spec.run_id,
                prepared.journal.completed_keys().len()
            );
        }

        let logger = match &args.log {
            Some(path) => Some(EventLogger::new(Some(path), args.log_format)?),
            None => None,
        };

        let artifact_dir = if args.no_artifacts {
            None
        } else {
            Some(
                base_dir
                    .join(&spec.run_dir_name)
                    .join(spec.run_id.to_string()),
            )
        };
        let artifact_writer = artifact_dir.map(|dir| ArtifactWriter::new(dir, spec.run_id));

        let result = run_headless(
            ctx,
            prepared.runtime,
            current_script.clone(),
            args.output.clone(),
            logger,
            artifact_writer,
            args.verbose,
        )
        .await;

        fwd.abort();

        match result {
            Ok(()) => {
                if let Some(ref stats) = mock_stats {
                    let snap = stats.snapshot();
                    if !snap.all_matched() {
                        anyhow::bail!(
                            "mock coverage incomplete: {} of {} agent calls unmatched{}",
                            snap.fallback,
                            snap.total_calls,
                            if snap.unmatched_names.is_empty() {
                                String::new()
                            } else {
                                format!(": {:?}", snap.unmatched_names)
                            }
                        );
                    }
                }
                return Ok(());
            }
            Err(e) if attempt < max_att => {
                let err_str = e.to_string();
                // Schema validation failures are LLM compliance issues, not
                // script bugs — auto-fix can't help and wastes a backend call.
                if err_str.contains("output schema validation failed") {
                    eprintln!("\u{2717} Schema validation failed (LLM output non-compliant); auto-fix skipped.");
                    return Err(e);
                }
                eprintln!(
                    "\u{26a0} Attempt {}/{} \u{2014} fixing script via LLM...",
                    attempt, max_att
                );
                match try_fix_script(&current_script, &e.to_string(), backend_id, model.clone())
                    .await
                {
                    Ok(fixed) => {
                        eprintln!("\u{2713} Fixed ({} bytes), retrying...", fixed.len());
                        current_script = fixed;
                        continue;
                    }
                    Err(fix_err) => {
                        eprintln!("Fix failed: {fix_err}");
                        return Ok(());
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// ── factory + reporting helpers ────────────────────────────────────────────

/// Create a backend, intercepting `mockfile` to load a `.mock.json` sidecar.
#[allow(clippy::type_complexity)]
fn create_run_backend(
    backend_id: &str,
    workflow_path: Option<&Path>,
    emit_raw: bool,
    model: Option<String>,
) -> Result<(Arc<dyn AgentBackend>, Option<Arc<MockStats>>)> {
    if backend_id == "mockfile" {
        let wf = workflow_path
            .ok_or_else(|| anyhow::anyhow!("--backend mockfile requires --workflow <file>"))?;
        let mock_path = wf.with_extension("mock.json");
        if !mock_path.exists() {
            anyhow::bail!(
                "mock file not found: {}. Run `maestro generate --with-mock` first.",
                mock_path.display()
            );
        }
        eprintln!("\u{2139}  Loading mock data from {}", mock_path.display());
        let mb = MockFileBackend::load(&mock_path)?;
        let stats = mb.stats_handle();
        Ok((Arc::new(mb), Some(stats)))
    } else {
        Ok((backend::create_backend(backend_id, emit_raw, model)?, None))
    }
}

/// Pull a human-readable string out of a run's final report value.
///
/// Returns `Some` only for:
///   * a JSON string (printed verbatim), or
///   * an object whose `markdown` field is a JSON string (printed verbatim —
///     so a research workflow can emit a clean `.md`).
///
/// Everything else (`null`, arrays, numbers, objects without a string
/// `markdown` field) returns `None` so callers can fall back to pretty JSON.
fn extract_report_text(report: &serde_json::Value) -> Option<String> {
    match report {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => match map.get("markdown") {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Persist the final report value to `path`. Delegates the
/// "string / markdown / pretty" decision to [`extract_report_text`] so the
/// rules can't drift between on-disk writes and [`print_report`].
fn write_report(path: &Path, report: &serde_json::Value) -> Result<()> {
    let content = extract_report_text(report).unwrap_or_else(|| {
        serde_json::to_string_pretty(report).unwrap_or_else(|_| report.to_string())
    });
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// Drain `rx`, handing each event to `handle`, until the terminal
/// [`AgentEvent::RunDone`] (or the channel closes). Returns the count of events
/// skipped because the bounded broadcast buffer lagged.
///
/// Draining concurrently with execution is what keeps the high-frequency
/// `acp_raw` stream from being dropped by the bounded bus.
async fn drain_events<F: FnMut(&AgentEvent)>(
    mut rx: tokio::sync::broadcast::Receiver<AgentEvent>,
    mut handle: F,
) -> u64 {
    use tokio::sync::broadcast::error::RecvError;
    let mut skipped = 0;
    loop {
        match rx.recv().await {
            // Raw ACP events are high-frequency noise in headless mode; skip
            // them so the printer stays fast enough to avoid broadcast lag.
            Ok(AgentEvent::AcpRaw { .. }) => {}
            Ok(evt) => {
                handle(&evt);
                if matches!(evt, AgentEvent::RunDone { .. }) {
                    break;
                }
            }
            Err(RecvError::Closed) => break,
            Err(RecvError::Lagged(n)) => skipped += n,
        }
    }
    skipped
}

/// Headless mode: drive both concurrent tasks ([`spawn_printer_task`] +
/// [`spawn_tick_task`]), run the script via the service layer, then
/// join/report via [`join_and_report`]. Returns `Ok(())` on successful
/// execution and `Err` otherwise so the caller can retry with a fixed script.
async fn run_headless(
    run_ctx: RunContext,
    rt: Runtime,
    script: String,
    output: Option<PathBuf>,
    logger: Option<EventLogger>,
    artifact_writer: Option<ArtifactWriter>,
    verbose: bool,
) -> Result<()> {
    let tty = console::user_attended();
    let renderer: Arc<Mutex<PhaseRenderer>> = Arc::new(Mutex::new(PhaseRenderer::new(tty)));
    let tool_calls: Arc<Mutex<HashMap<AgentId, Vec<String>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let printer = spawn_printer_task(
        run_ctx.events.subscribe(),
        renderer.clone(),
        tool_calls.clone(),
        logger,
        artifact_writer,
    );

    let (stop_tick_tx, stop_tick_rx) = tokio::sync::oneshot::channel::<()>();
    let tick = spawn_tick_task(renderer, stop_tick_rx);

    join_and_report(
        printer,
        tick,
        stop_tick_tx,
        run_ctx,
        rt,
        script,
        output,
        tool_calls,
        verbose,
    )
    .await
}

/// Subscribe to the run's event bus and drive the renderer + optional
/// logger + optional artifact writer. Records tool-call names into
/// `tool_calls` for the verbose summary.
///
/// Returns a join handle whose inner value is the count of events dropped
/// because the bounded broadcast bus lagged (the caller logs that as a
/// warning).
#[allow(clippy::too_many_arguments)]
fn spawn_printer_task(
    rx: tokio::sync::broadcast::Receiver<AgentEvent>,
    renderer: Arc<Mutex<PhaseRenderer>>,
    tool_calls: Arc<Mutex<HashMap<AgentId, Vec<String>>>>,
    mut logger: Option<EventLogger>,
    mut artifact_writer: Option<ArtifactWriter>,
) -> tokio::task::JoinHandle<u64> {
    use maestro::core::contract::event::ProgressDelta;
    tokio::spawn(async move {
        let skipped = drain_events(rx, |evt| {
            if let AgentEvent::AgentProgress {
                agent_id,
                delta: ProgressDelta::ToolCall { ref name, .. },
                ..
            } = evt
            {
                tool_calls
                    .lock()
                    .unwrap()
                    .entry(*agent_id)
                    .or_default()
                    .push(name.clone());
            }
            renderer.lock().unwrap().handle(evt);
            if let Some(l) = logger.as_mut() {
                let _ = l.write(evt);
            }
            if let Some(w) = artifact_writer.as_mut() {
                w.handle(evt);
            }
        })
        .await;
        if let Some(l) = logger.as_mut() {
            let _ = l.flush();
        }
        skipped
    })
}

/// Live wall-clock timer task: every [`TICK_INTERVAL_SECS`], ask the shared
/// renderer to update its `⏱ …` header suffix. Stops cleanly when `stop`
/// fires — that's how we avoid one more re-render against a header that
/// just printed its final `╰─ Run done` line.
fn spawn_tick_task(
    renderer: Arc<Mutex<PhaseRenderer>>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(TICK_INTERVAL_SECS));
        // Skip the immediate first tick — `RunStarted` itself draws the
        // initial `⏱ 0s`, and we don't want a redundant re-render at t=0.
        interval.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = &mut stop => break,
                _ = interval.tick() => {
                    renderer.lock().unwrap().tick_elapsed();
                }
            }
        }
    })
}

/// Drive [`svc::execute`], join both concurrent tasks with bounded
/// timeouts, and surface the run report. Delegates the "string /
/// markdown / pretty" decision to [`extract_report_text`] (shared with
/// [`write_report`]) so the on-disk and on-screen rendering can't drift.
///
/// The long parameter list is the cost of cleanly orchestrating execution +
/// join + reporting without leaking ad-hoc state into a config struct.
#[allow(clippy::too_many_arguments)]
async fn join_and_report(
    printer: tokio::task::JoinHandle<u64>,
    tick: tokio::task::JoinHandle<()>,
    stop_tick_tx: tokio::sync::oneshot::Sender<()>,
    run_ctx: RunContext,
    rt: Runtime,
    script: String,
    output: Option<PathBuf>,
    tool_calls: Arc<Mutex<HashMap<AgentId, Vec<String>>>>,
    verbose: bool,
) -> Result<()> {
    let exec_result = svc::execute(&run_ctx, rt, script).await;

    if let Ok(Ok(skipped)) =
        tokio::time::timeout(Duration::from_secs(PRINT_JOIN_TIMEOUT_SECS), printer).await
    {
        // Printer drained — the final `RunDone` event has been painted. Tell
        // the tick task to exit so it doesn't fire one more `set_message`
        // against a finished bar.
        let _ = stop_tick_tx.send(());
        let _ = tokio::time::timeout(Duration::from_secs(TICK_JOIN_TIMEOUT_SECS), tick).await;
        if skipped > 0 {
            eprintln!("⚠ event stream lagged, skipped {skipped} events");
        }
    } else {
        // Printer didn't finish in time; drop the stop signal (oneshot
        // closure) and abandon the tick task. It will be cancelled when its
        // parent future is dropped at function return.
        drop(stop_tick_tx);
    }

    let result = exec_result?;
    match result {
        Ok(report) => {
            if let Some(path) = &output {
                write_report(path, &report)?;
                eprintln!("Report written to {}", path.display());
            }
            if verbose {
                print_verbose_summary(&tool_calls);
            }
            print_report(&report);
            Ok(())
        }
        Err(e) => {
            tracing::error!(error = %e, "workflow execution failed");
            eprintln!("Execution error: {}", e);
            Err(anyhow::anyhow!("{}", e))
        }
    }
}

/// Print the final report value to stdout in a human-readable way.
///
/// String reports (and objects with a `markdown` string field) are printed
/// verbatim; everything else is pretty-printed as JSON. `null` is silently
/// skipped — workflows that emit no final report are valid.
fn print_report(report: &serde_json::Value) {
    if let Some(text) = extract_report_text(report) {
        println!();
        println!("{text}");
    } else if !matches!(report, serde_json::Value::Null) {
        println!();
        println!(
            "{}",
            serde_json::to_string_pretty(report).unwrap_or_else(|_| report.to_string())
        );
    }
}

/// Call the LLM backend to fix a broken Lua workflow script.
async fn try_fix_script(
    script: &str,
    error: &str,
    backend_id: &str,
    model: Option<String>,
) -> Result<String> {
    use maestro::core::contract::backend::AgentTask;
    use maestro::core::contract::ids::AgentId;
    use maestro::core::RunContext;

    let backend = crate::backend::create_backend(backend_id, false, model)?;
    let prompt = format!(
        "The following Lua workflow script failed during execution:\n\n\
         --- Error ---\n{error}\n\n\
         --- Script ---\n```lua\n{script}\n```\n\n\
         Please fix the script. Return ONLY the fixed Lua code wrapped in ```lua ... ```."
    );

    let task = AgentTask {
        agent_id: AgentId::now_v7(),
        phase_id: 0,
        prompt,
        model: None,
        description: None,
        role: None,
        name: None,
        agent_seq: 0,
        allowlist: None,
        workdir: std::env::current_dir().unwrap_or_default(),
        mcp_endpoint: None,
        timeout: Some(std::time::Duration::from_secs(FIX_LUA_TIMEOUT_SECS)),
        output_schema: None,
    };

    use maestro::core::contract::event::AgentEvent;
    let (tx, _rx) = tokio::sync::broadcast::channel::<AgentEvent>(FIX_CHANNEL_CAPACITY);
    let ctx = RunContext {
        run_id: AgentId::now_v7(),
        cancel: CancellationToken::new(),
        events: tx,
    };

    let result = backend
        .run(task, ctx)
        .await
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;

    if result.status != maestro::core::contract::backend::AgentStatus::Ok {
        anyhow::bail!("backend returned status {:?}", result.status);
    }

    let content = match &result.output {
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    };

    // Simple Lua code extraction: find ```lua ... ``` block
    if let Some(start) = content.find("```lua") {
        let rest = &content[start + 6..];
        if let Some(end) = rest.find("```") {
            return Ok(rest[..end].trim().to_string());
        }
    }
    // Fallback: try the whole output as Lua code
    Ok(content.to_string())
}

fn print_verbose_summary(tool_calls: &Arc<Mutex<HashMap<AgentId, Vec<String>>>>) {
    let calls = tool_calls.lock().unwrap();
    if calls.is_empty() {
        println!();
        println!("=== Structured Output Summary ===");
        println!("No tool calls recorded.");
        return;
    }

    let total_agents = calls.len();
    let structured_agents: Vec<(&AgentId, &Vec<String>)> = calls
        .iter()
        .filter(|(_, names)| names.iter().any(|n| n == "structured_output"))
        .collect();

    println!();
    println!("=== Structured Output Summary ===");
    println!("Agents with tool calls: {total_agents}");
    println!(
        "Agents that called structured_output: {}",
        structured_agents.len()
    );

    for (agent_id, names) in calls.iter() {
        let called = names.iter().any(|n| n == "structured_output");
        let all_tools = names.join(", ");
        let mark = if called { "\u{2713}" } else { "\u{2717}" };
        println!("  {agent_id} {mark}  tools: {all_tools}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::event_log::LogFormat;
    use maestro::core::contract::event::{ProgressDelta, RunStatus};
    use maestro::core::contract::ids::{RunId, TokenUsage};
    use maestro::core::{BackendRegistry, Scheduler, SchedulerConfig};
    use maestro::runtime::{ExecLimits, Runtime};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn progress(run_id: RunId, text: &str) -> AgentEvent {
        AgentEvent::AgentProgress {
            run_id,
            agent_id: run_id,
            delta: ProgressDelta::Message { text: text.into() },
        }
    }

    fn run_done(run_id: RunId) -> AgentEvent {
        AgentEvent::RunDone {
            run_id,
            status: RunStatus::Completed,
            total_tokens: TokenUsage::default(),
            report: serde_json::json!({ "ok": true }),
        }
    }

    async fn empty_script_runtime(run_ctx: &RunContext) -> Runtime {
        let registry = BackendRegistry::new();
        let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
        scheduler.init_run_with(run_ctx.run_id, run_ctx.events.clone());

        let handle = tokio::runtime::Handle::current();
        Runtime::new(
            scheduler,
            run_ctx.clone(),
            serde_json::json!({}),
            ExecLimits::default(),
            None,
            handle,
        )
        .expect("runtime init")
    }

    // ── drain_events ───────────────────────────────────────────

    #[tokio::test]
    async fn drain_stops_on_run_done_and_ignores_later_events() {
        let (tx, rx) = tokio::sync::broadcast::channel(16);
        let run_id = RunId::now_v7();
        tx.send(progress(run_id, "hi")).unwrap();
        tx.send(run_done(run_id)).unwrap();
        tx.send(progress(run_id, "after")).unwrap();

        let mut lines = Vec::new();
        let skipped = drain_events(rx, |evt| lines.push(serde_json::to_string(evt).unwrap())).await;

        assert_eq!(skipped, 0);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"type\":\"agent_progress\""));
        assert!(lines[1].contains("\"type\":\"run_done\""));
    }

    #[tokio::test]
    async fn drain_skips_acp_raw() {
        let (tx, rx) = tokio::sync::broadcast::channel(16);
        let run_id = RunId::now_v7();
        tx.send(AgentEvent::AcpRaw {
            run_id,
            agent_id: run_id,
            kind: "plan".into(),
            raw: serde_json::json!({ "sessionUpdate": "plan" }),
        })
        .unwrap();
        tx.send(run_done(run_id)).unwrap();

        let mut lines = Vec::new();
        drain_events(rx, |evt| lines.push(serde_json::to_string(evt).unwrap())).await;

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("\"type\":\"run_done\""));
    }

    #[tokio::test]
    async fn drain_counts_lagged_but_still_terminates() {
        let (tx, rx) = tokio::sync::broadcast::channel(2);
        let run_id = RunId::now_v7();
        for i in 0..4 {
            tx.send(progress(run_id, &format!("m{i}"))).unwrap();
        }
        tx.send(run_done(run_id)).unwrap();

        let mut lines = Vec::new();
        let skipped = drain_events(rx, |evt| lines.push(serde_json::to_string(evt).unwrap())).await;

        assert!(skipped > 0, "expected lagged events, got {skipped}");
        assert!(lines.last().unwrap().contains("\"type\":\"run_done\""));
    }

    #[tokio::test]
    async fn drain_stops_when_channel_closes() {
        let (tx, rx) = tokio::sync::broadcast::channel(16);
        let run_id = RunId::now_v7();
        tx.send(progress(run_id, "m1")).unwrap();
        tx.send(progress(run_id, "m2")).unwrap();
        drop(tx);

        let mut lines = Vec::new();
        let skipped = drain_events(rx, |evt| lines.push(serde_json::to_string(evt).unwrap())).await;

        assert_eq!(skipped, 0);
        assert_eq!(lines.len(), 2);
    }

    // ── extract_report_text ───────────────────────────────────

    #[test]
    fn extract_string_verbatim() {
        assert_eq!(
            extract_report_text(&serde_json::json!("hello world")),
            Some("hello world".to_string())
        );
    }

    #[test]
    fn extract_markdown_field_verbatim() {
        assert_eq!(
            extract_report_text(&serde_json::json!({"markdown": "# Title\nbody"})),
            Some("# Title\nbody".to_string())
        );
    }

    #[test]
    fn extract_null_returns_none() {
        assert_eq!(extract_report_text(&serde_json::json!(null)), None);
    }

    #[test]
    fn extract_non_string_markdown_returns_none() {
        assert_eq!(
            extract_report_text(&serde_json::json!({"markdown": 42, "other": "v"})),
            None
        );
    }

    #[test]
    fn extract_object_without_markdown_returns_none() {
        assert_eq!(extract_report_text(&serde_json::json!({"a": 1})), None);
    }

    #[test]
    fn extract_array_returns_none() {
        assert_eq!(extract_report_text(&serde_json::json!([1, 2, 3])), None);
    }

    #[test]
    fn extract_primitive_returns_none() {
        assert_eq!(extract_report_text(&serde_json::json!(42)), None);
        assert_eq!(extract_report_text(&serde_json::json!(true)), None);
    }

    // ── write_report ──────────────────────────────────────────

    #[test]
    fn write_report_string_verbatim() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.txt");
        write_report(&path, &serde_json::json!("hello world")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[test]
    fn write_report_markdown_field() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.md");
        write_report(&path, &serde_json::json!({"markdown": "# Title\nbody"})).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Title\nbody");
    }

    #[test]
    fn write_report_markdown_not_string() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.txt");
        write_report(&path, &serde_json::json!({"markdown": 42, "other": "val"})).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"markdown\""));
        assert!(content.contains("42"));
        assert!(content.contains("\"other\""));
    }

    #[test]
    fn write_report_object_without_markdown() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.txt");
        write_report(&path, &serde_json::json!({"a": 1, "b": 2})).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"a\""));
        assert!(content.contains("1"));
    }

    #[test]
    fn write_report_array() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.txt");
        write_report(&path, &serde_json::json!([10, 20, 30])).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("["));
        assert!(content.contains("10"));
    }

    #[test]
    fn write_report_primitives() {
        let dir = TempDir::new().unwrap();
        for (label, val, expected) in [
            ("number", serde_json::json!(42), "42"),
            ("bool", serde_json::json!(true), "true"),
            ("null", serde_json::json!(null), "null"),
        ] {
            let path = dir.path().join(format!("out_{label}.txt"));
            write_report(&path, &val).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), expected);
        }
    }

    #[test]
    fn write_report_creates_parent_dir() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/sub/dir/report.txt");
        write_report(&path, &serde_json::json!("deeply nested")).unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "deeply nested");
    }

    #[test]
    fn write_report_existing_parent_dir() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("direct.txt");
        write_report(&path, &serde_json::json!("direct")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "direct");
    }

    #[test]
    fn write_report_flat_filename_empty_parent() {
        let id = RunId::now_v7();
        let name = format!("__maestro_test_write_report_{id}.tmp");
        let path = Path::new(&name);
        write_report(path, &serde_json::json!("flat")).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "flat");
        std::fs::remove_file(path).ok();
    }

    // ── run_headless ──────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread")]
    async fn run_headless_empty_script_with_output() {
        let tmp = TempDir::new().unwrap();
        let output = tmp.path().join("report.md");
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(EVENT_BUS_CAPACITY);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let rt = empty_script_runtime(&run_ctx).await;

        run_headless(
            run_ctx,
            rt,
            "function main() end".to_string(),
            Some(output.clone()),
            None,
            None,
            false,
        )
        .await
        .unwrap();

        assert!(output.exists());
        let content = std::fs::read_to_string(&output).unwrap();
        assert_eq!(content.trim(), "null");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_headless_empty_script_no_output() {
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(EVENT_BUS_CAPACITY);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let rt = empty_script_runtime(&run_ctx).await;

        run_headless(run_ctx, rt, "function main() end".to_string(), None, None, None, false)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_headless_script_error_returns_err() {
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(EVENT_BUS_CAPACITY);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let rt = empty_script_runtime(&run_ctx).await;

        let result =
            run_headless(run_ctx, rt, "bad lua".to_string(), None, None, None, false).await;
        assert!(result.is_err(), "expected Err on script failure");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("syntax error") || err.contains("Syntax"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_headless_with_event_logger() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("events.jsonl");
        let logger = EventLogger::new(Some(&log_path), LogFormat::Jsonl).unwrap();

        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(EVENT_BUS_CAPACITY);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let rt = empty_script_runtime(&run_ctx).await;

        run_headless(run_ctx, rt, "function main() end".to_string(), None, Some(logger), None, false)
            .await
            .unwrap();

        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(!content.is_empty(), "logger should have written events");
        assert!(content.contains("\"type\":\"run_done\""));
    }
}
