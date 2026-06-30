//! `run` subcommand: detect the backend, resolve (and optionally confirm) the
//! script via the shared run service ([`maestro::service::run`]), then drive
//! output. This is the sole presentation layer for `run`;
//! the library exposes only the presentation-free service.

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
use maestro::runtime::Runtime;
use maestro::service::run as svc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub async fn run_workflow(args: RunArgs) -> Result<()> {
    let is_nl = args.nl.is_some();
    let backend_id = crate::config::resolve_default_backend(args.backend.as_deref());
    if is_nl && backend_id == "mock" {
        anyhow::bail!(
            "NL mode requires a real LLM backend. \
             Install opencode (https://opencode.ai) or specify --backend <id>"
        );
    }
    if is_nl && args.backend.is_none() {
        eprintln!(
            "\u{2139}  no --backend specified, auto-detected: {}",
            backend_id
        );
    }
    let base_dir = runs_base_dir();

    // Resolve a fully-specified run (script + run id + resume flag).
    let mut spec = if args.resume {
        let run_dir = svc::latest_resumable(&base_dir)?;
        svc::resolve_resume(&run_dir, &base_dir)?
    } else {
        let backend = backend::create_backend(&backend_id, !args.no_acp_raw)?;
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
        let mut s = svc::resolve_fresh(source, backend).await?;
        svc::assign_dir_name(&mut s, &base_dir);
        s
    };
    // `--args` takes precedence over the positional `extra_args`.
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

    let max_att = if args.auto_fix {
        args.max_fix_attempts.max(1)
    } else {
        1
    };
    let mut current_script = spec.script.clone();

    for attempt in 1..=max_att {
        let cancel = tokio_util::sync::CancellationToken::new();
        let (tx, _rx) = tokio::sync::broadcast::channel(2048);
        let ctx = RunContext {
            run_id: spec.run_id,
            cancel,
            events: tx,
        };

        let backend2 = backend::create_backend(&backend_id, !args.no_acp_raw)?;

        let prepared = svc::prepare(&spec, backend2, &base_dir, &ctx).await?;

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

        match result {
            Ok(()) => return Ok(()),
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
                match try_fix_script(&current_script, &e.to_string(), &backend_id).await {
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

/// Headless mode: render events as a live phase tree to stdout (concurrently
/// with execution), then print the final report. When `logger` is set, each
/// event is also written to the event-log sink.
///
/// Returns `Ok(())` on successful execution. Returns an error when execution
/// fails, so the caller can retry with a fixed script.
async fn run_headless(
    run_ctx: RunContext,
    rt: Runtime,
    script: String,
    output: Option<PathBuf>,
    mut logger: Option<EventLogger>,
    mut artifact_writer: Option<ArtifactWriter>,
    verbose: bool,
) -> Result<()> {
    use maestro::core::contract::event::ProgressDelta;
    use std::collections::HashMap;
    use tokio::time::Duration;

    let tty = console::user_attended();
    let rx = run_ctx.events.subscribe();
    let tool_calls: Arc<Mutex<HashMap<AgentId, Vec<String>>>> = Arc::new(Mutex::new(HashMap::new()));
    let tool_calls_clone = tool_calls.clone();
    let printer = tokio::spawn(async move {
        let mut renderer = PhaseRenderer::new(tty);
        let skipped = drain_events(rx, |evt| {
            if let AgentEvent::AgentProgress {
                agent_id,
                delta: ProgressDelta::ToolCall { ref name, .. },
                ..
            } = evt
            {
                tool_calls_clone
                    .lock()
                    .unwrap()
                    .entry(*agent_id)
                    .or_default()
                    .push(name.clone());
            }
            renderer.handle(evt);
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
    });

    let exec_result = svc::execute(&run_ctx, rt, script).await;

    if let Ok(Ok(skipped)) = tokio::time::timeout(Duration::from_secs(2), printer).await {
        if skipped > 0 {
            eprintln!("\u{26a0} event stream lagged, skipped {skipped} events");
        }
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
/// skipped.
fn print_report(report: &serde_json::Value) {
    match report {
        serde_json::Value::Null => {}
        serde_json::Value::String(s) => {
            println!();
            println!("{s}");
        }
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get("markdown") {
                println!();
                println!("{s}");
            } else {
                println!();
                println!(
                    "{}",
                    serde_json::to_string_pretty(report).unwrap_or_default()
                );
            }
        }
        other => {
            println!();
            println!(
                "{}",
                serde_json::to_string_pretty(other).unwrap_or_default()
            );
        }
    }
}

/// Call the LLM backend to fix a broken Lua workflow script.
async fn try_fix_script(script: &str, error: &str, backend_id: &str) -> Result<String> {
    use maestro::core::contract::backend::AgentTask;
    use maestro::core::contract::ids::AgentId;
    use maestro::core::RunContext;
    use tokio_util::sync::CancellationToken;

    let backend = crate::backend::create_backend(backend_id, false)?;
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
        timeout: Some(std::time::Duration::from_secs(60)),
        output_schema: None,
    };

    use maestro::core::contract::event::AgentEvent;
    let (tx, _rx) = tokio::sync::broadcast::channel::<AgentEvent>(8);
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
    println!("Agents that called structured_output: {}", structured_agents.len());

    for (agent_id, names) in calls.iter() {
        let called = names.iter().any(|n| n == "structured_output");
        let all_tools = names.join(", ");
        let mark = if called { "✓" } else { "✗" };
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
        let (tx, _rx) = tokio::sync::broadcast::channel(256);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let rt = empty_script_runtime(&run_ctx).await;

        run_headless(
            run_ctx,
            rt,
            "".to_string(),
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
        let (tx, _rx) = tokio::sync::broadcast::channel(256);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let rt = empty_script_runtime(&run_ctx).await;

        run_headless(run_ctx, rt, "".to_string(), None, None, None, false)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_headless_script_error_returns_err() {
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(256);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let rt = empty_script_runtime(&run_ctx).await;

        let result = run_headless(
            run_ctx,
            rt,
            "bad lua".to_string(),
            None,
            None,
            None,
            false,
        )
        .await;
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
        let (tx, _rx) = tokio::sync::broadcast::channel(256);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let rt = empty_script_runtime(&run_ctx).await;

        run_headless(run_ctx, rt, "".to_string(), None, Some(logger), None, false)
            .await
            .unwrap();

        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(!content.is_empty(), "logger should have written events");
        assert!(content.contains("\"type\":\"run_done\""));
    }
}
