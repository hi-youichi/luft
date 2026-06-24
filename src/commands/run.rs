//! `run` subcommand: detect the backend, resolve (and optionally confirm) the
//! script via the shared run service ([`maestro::service::run`]), then drive
//! output. This is the sole presentation layer for `run`;
//! the library exposes only the presentation-free service.

use crate::backend;
use crate::commands::event_log::EventLogger;
use crate::commands::runs_base_dir;
use crate::RunArgs;
use anyhow::Result;
use maestro::core::contract::backend::RunContext;
use maestro::core::contract::event::AgentEvent;
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
    let backend = backend::create_backend(&backend_id, !args.no_acp_raw)?;
    let base_dir = runs_base_dir();

    // Resolve a fully-specified run (script + run id + resume flag). NL planning,
    // workflow-file reads and script pass-through all live in the service; this
    // resolves exactly once (so `--confirm` shows the same script that runs).
    let mut spec = if args.resume {
        let run_dir = svc::latest_resumable(&base_dir)?;
        svc::resolve_resume(&run_dir, &base_dir)?
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
        let mut s = svc::resolve_fresh(source, backend.clone()).await?;
        svc::assign_dir_name(&mut s, &base_dir);
        s
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
    // headless output).
    let (events_tx, _events_rx) = tokio::sync::broadcast::channel(256);
    let run_ctx = RunContext {
        run_id: spec.run_id,
        cancel: tokio_util::sync::CancellationToken::new(),
        events: events_tx,
    };

    let prepared = svc::prepare(&spec, backend, &base_dir, &run_ctx).await?;
    if spec.resuming {
        println!(
            "Resuming run {} ({} agents cached)",
            spec.run_id,
            prepared.journal.completed_keys().len()
        );
    }

    // Optional event-log sink.
    let logger = match &args.log {
        Some(path) => Some(EventLogger::new(Some(path), args.log_format)?),
        None => None,
    };

    run_headless(run_ctx, prepared.runtime, spec.script, args.output, logger).await
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
            Ok(evt) => {
                handle(&evt);
                // RunDone is the terminal event — stop once it's been handled.
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

/// Headless mode: stream events as JSONL live (concurrently with execution),
/// then the final report. When `logger` is set, each event is also written to
/// the event-log sink.
async fn run_headless(
    run_ctx: RunContext,
    rt: Runtime,
    script: String,
    output: Option<PathBuf>,
    mut logger: Option<EventLogger>,
) -> Result<()> {
    use tokio::time::Duration;

    let run_id = run_ctx.run_id;

    // Stream events live so the bounded broadcast buffer can't drop the
    // high-frequency acp_raw stream. The printer exits on the terminal RunDone.
    let rx = run_ctx.events.subscribe();
    let printer = tokio::spawn(async move {
        let skipped = drain_events(rx, |evt| {
            if let Ok(s) = serde_json::to_string(evt) {
                println!("{s}");
            }
            if let Some(l) = logger.as_mut() {
                let _ = l.write(evt);
            }
        })
        .await;
        if let Some(l) = logger.as_mut() {
            let _ = l.flush();
        }
        skipped
    });

    let exec_result = svc::execute(&run_ctx, rt, script).await;

    // RunDone is emitted inside execute() before it returns, so the printer
    // sees it and stops; the timeout only guards against a dropped RunDone.
    // Always drain before propagating errors, otherwise the printer task is
    // dropped before it can process the terminal RunDone event.
    if let Ok(Ok(skipped)) = tokio::time::timeout(Duration::from_secs(2), printer).await {
        if skipped > 0 {
            eprintln!("⚠ event stream lagged, skipped {skipped} events");
        }
    }

    let result = exec_result?;
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
        Err(e) => {
            tracing::error!(error = %e, "workflow execution failed");
            eprintln!("Execution error: {}", e);
        }
    }
    Ok(())
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
        ts: chrono::Utc::now(),
        }
    }

    /// Build a Runtime suitable for passing to `run_headless` with an empty
    /// script (no SDK primitives called, so an empty backend registry is fine).
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
        tx.send(progress(run_id, "after")).unwrap(); // must not be emitted

        let mut lines = Vec::new();
        let skipped = drain_events(rx, |evt| lines.push(serde_json::to_string(evt).unwrap())).await;

        assert_eq!(skipped, 0);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"type\":\"agent_progress\""));
        assert!(lines[1].contains("\"type\":\"run_done\""));
    }

    #[tokio::test]
    async fn drain_passes_through_acp_raw() {
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

        assert!(lines[0].contains("\"type\":\"acp_raw\""));
        assert!(lines[0].contains("\"kind\":\"plan\""));
    }

    #[tokio::test]
    async fn drain_counts_lagged_but_still_terminates() {
        // Cap-2 channel, overfilled before draining → early events are dropped.
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
        drop(tx); // close the channel so the next recv() returns Closed

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
        // "markdown" is not a string → falls back to pretty-printed JSON of the whole object
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

        run_headless(run_ctx, rt, "".to_string(), Some(output.clone()), None)
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

        run_headless(run_ctx, rt, "".to_string(), None, None)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_headless_script_error_path() {
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(256);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let rt = empty_script_runtime(&run_ctx).await;

        run_headless(run_ctx, rt, "not valid lua <<<>>>".to_string(), None, None)
            .await
            .unwrap();
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

        run_headless(run_ctx, rt, "".to_string(), None, Some(logger))
            .await
            .unwrap();

        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(!content.is_empty(), "logger should have written events");
        assert!(content.contains("\"type\":\"run_done\""));
    }
}
