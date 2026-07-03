//! Source-agnostic event logging — see `docs/design/event-logging.md`.
//!
//! [`EventLogger`] consumes [`AgentEvent`]s and writes one line each, in either
//! human-readable (`pretty`) or machine (`jsonl`) form. Both the local `run`
//! path feed the same logger, so the
//! log is identical regardless of which end observed the stream.

use anyhow::Result;
use maestro::core::contract::event::{AgentEvent, ProgressDelta};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Output format for [`EventLogger`].
#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
pub enum LogFormat {
    /// Human-readable one-line-per-event summary.
    #[default]
    Pretty,
    /// One `serde_json` line per event — byte-identical to `events.jsonl`.
    Jsonl,
}

/// Writes an event stream to a sink (file or stdout) in the chosen format.
///
/// Buffered: the underlying [`BufWriter`] batches writes (so a high-frequency
/// `acp_raw` stream doesn't cause a syscall per line); [`Self::flush`] forces a
/// flush, and the buffer is also flushed on drop.
pub struct EventLogger {
    sink: BufWriter<Box<dyn Write + Send>>,
    format: LogFormat,
}

impl EventLogger {
    /// `out = None` → stdout; `Some(path)` → append to the file (created if absent).
    pub fn new(out: Option<&Path>, format: LogFormat) -> Result<Self> {
        let inner: Box<dyn Write + Send> = match out {
            Some(p) => Box::new(File::options().create(true).append(true).open(p)?),
            None => Box::new(std::io::stdout()),
        };
        Ok(Self {
            sink: BufWriter::new(inner),
            format,
        })
    }

    /// Format and write one event as a line.
    pub fn write(&mut self, evt: &AgentEvent) -> Result<()> {
        match self.format {
            LogFormat::Jsonl => writeln!(self.sink, "{}", serde_json::to_string(evt)?)?,
            LogFormat::Pretty => writeln!(self.sink, "{}", format_event_line(evt))?,
        }
        Ok(())
    }

    /// Flush buffered lines to the sink.
    pub fn flush(&mut self) -> Result<()> {
        self.sink.flush()?;
        Ok(())
    }

    #[cfg(test)]
    pub fn new_with_writer(writer: Box<dyn Write + Send>, format: LogFormat) -> Self {
        Self {
            sink: BufWriter::new(writer),
            format,
        }
    }
}

/// One-line human summary of an event. **Exhaustive** over `AgentEvent` (no
/// `_` arm) so a new variant fails to compile until it is given a rendering.
pub fn format_event_line(evt: &AgentEvent) -> String {
    use AgentEvent::*;
    match evt {
        RunStarted { task, .. } => format!("run started: {task}"),
        PhaseStarted { phase_id, label, planned, .. } => {
            format!("phase {phase_id} started: {label} ({planned} planned)")
        }
        AgentStarted { agent_id, model, name, description, .. } => {
            let fallback = agent_id.to_string();
            let label = name.as_deref().or(description.as_deref()).unwrap_or(&fallback);
            format!("agent {} started (model {})", label, model.as_deref().unwrap_or("default"))
        }
        AgentProgress { agent_id, delta, .. } => format!("agent {agent_id} · {}", format_delta(delta)),
        AcpRaw { kind, .. } => format!("acp raw: {kind}"),
        AgentDone { agent_id, status, tokens, elapsed_ms, name, .. } => {
            let fallback = agent_id.to_string();
            let label = name.as_deref().unwrap_or(&fallback);
            format!("agent {} done: {status:?} ({elapsed_ms}ms, {} tok)", label, tokens.display_total())
        }
        PhaseDone { phase_id, ok, failed, .. } => {
            format!("phase {phase_id} done: {ok} ok, {failed} failed")
        }
        RunDone { status, total_tokens, .. } => {
            format!("run done: {status:?} ({} tok)", total_tokens.display_total())
        }
        Log { level, msg, .. } => format!("log [{level:?}] {msg}"),
        BudgetSet { time_limit_ms, max_rounds, .. } => {
            format!("budget set: time={time_limit_ms:?}ms rounds={max_rounds:?}")
        }
        ReportEmitted { .. } => "report emitted".to_string(),
        ParallelStarted { span_id, count, .. } => {
            format!("parallel#{span_id} started: {count} items")
        }
        ParallelDone { span_id, ok, failed, elapsed_ms, .. } => {
            format!("parallel#{span_id} done: {ok} ok, {failed} failed ({elapsed_ms}ms)")
        }
        WorkflowStarted { span_id, path, .. } => format!("workflow#{span_id} started: {path}"),
        WorkflowDone { span_id, path, elapsed_ms, error, .. } => match error {
            Some(e) => format!("workflow#{span_id} failed: {path} ({elapsed_ms}ms): {e}"),
            None => format!("workflow#{span_id} done: {path} ({elapsed_ms}ms)"),
        },
        ConvergeStarted { span_id, items, max_rounds, .. } => {
            format!("converge#{span_id} started: {items} items, max {max_rounds} rounds")
        }
        ConvergeDone { span_id, rounds, converged, surviving, elapsed_ms, error, .. } => match error {
            Some(e) => format!("converge#{span_id} failed ({elapsed_ms}ms): {e}"),
            None => format!(
                "converge#{span_id} done: {rounds} rounds, converged={converged}, {surviving} surviving ({elapsed_ms}ms)"
            ),
        },
        PipelineStarted { total_stages, items, .. } => {
            format!("pipeline started: {total_stages} stages, {items} items")
        }
        PipelineStageStarted { stage_index, label, agents_in_stage, .. } => {
            format!("pipeline stage {stage_index} started: {label} ({agents_in_stage} agents)")
        }
        PipelineItemDone { stage_index, item_index, status, elapsed_ms, .. } => {
            format!("pipeline stage {stage_index} item {item_index} done: {status:?} ({elapsed_ms}ms)")
        }
        PipelineDone { stages_completed, total_ok, total_failed, .. } => {
            format!("pipeline done: {stages_completed} stages, {total_ok} ok, {total_failed} failed")
        }
        PhaseSpanStarted { span_id, name, depth, .. } => {
            format!("phase span#{span_id} started (depth {depth}): {name}")
        }
        PhaseSpanDone { span_id, name, elapsed_ms, status, .. } => {
            format!("phase span#{span_id} done: {name} ({status}, {elapsed_ms}ms)")
        }
        PlanPreview { reasoning, phases, .. } => {
            let labels: Vec<String> = phases.iter().map(|p| {
                if p.dynamic {
                    format!("{} (dynamic)", p.label)
                } else {
                    p.label.clone()
                }
            }).collect();
            format!("plan preview: {reasoning} | phases: {}", labels.join(", "))
        }
    }
}

fn format_delta(delta: &ProgressDelta) -> String {
    match delta {
        ProgressDelta::Message { text } => format!("msg: {}", truncate(text, 80)),
        ProgressDelta::ToolCall { name, summary } => format!("tool: {name} {summary}"),
        ProgressDelta::FileEdit { path } => format!("edit: {}", path.display()),
        ProgressDelta::Tokens { usage } => format!("tokens: {}", usage.display_total()),
    }
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro::core::contract::backend::AgentStatus;
    use maestro::core::contract::event::LogLevel;
    use maestro::core::contract::event::ProgressDelta;
    use maestro::core::contract::event::RunStatus;
    use maestro::core::contract::ids::{RunId, TokenUsage};
    use std::path::PathBuf;

    fn rid() -> RunId {
        RunId::now_v7()
    }

    #[test]
    fn jsonl_matches_serde_to_string() {
        let evt = AgentEvent::PhaseDone {
            run_id: rid(),
            phase_id: 2,
            ok: 1,
            failed: 0,
        };
        let dir = std::env::temp_dir().join(format!("maestro_evlog_{}", uuid::Uuid::now_v7()));
        let mut logger = EventLogger::new(Some(&dir), LogFormat::Jsonl).unwrap();
        logger.write(&evt).unwrap();
        logger.flush().unwrap();
        let written = std::fs::read_to_string(&dir).unwrap();
        assert_eq!(written.trim_end(), serde_json::to_string(&evt).unwrap());
        std::fs::remove_file(&dir).ok();
    }

    #[test]
    fn truncate_caps_and_strips_newlines() {
        assert_eq!(truncate("a\nb", 10), "a b");
        assert!(truncate(&"x".repeat(200), 80).ends_with('…'));
    }

    #[test]
    fn truncate_empty_string() {
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn truncate_zero_max() {
        assert_eq!(truncate("hello", 0), "…");
        assert_eq!(truncate("", 0), "");
    }

    #[test]
    fn truncate_multibyte_unicode() {
        let s = "\u{1f980}\u{1f980}\u{1f980}\u{1f980}\u{1f980}";
        assert_eq!(truncate(s, 3), "\u{1f980}\u{1f980}\u{1f980}\u{2026}");
        assert_eq!(truncate(s, 5), s);
        assert!(truncate(s, 4).ends_with('\u{2026}'));
    }

    #[test]
    fn event_logger_new_stdout_and_pretty() {
        let r = rid();
        let evt = AgentEvent::RunStarted {
            run_id: r,
            task: "test".into(),
            ts: chrono::Utc::now(),
        };
        let mut logger = EventLogger::new(None, LogFormat::Pretty).unwrap();
        assert_eq!(format_event_line(&evt), "run started: test");
        logger.write(&evt).unwrap();
        logger.flush().unwrap();
    }

    #[test]
    fn format_covers_all_variants() {
        let r = rid();
        let t = TokenUsage {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
        };
        let cases: Vec<(AgentEvent, &str)> = vec![
            (
                AgentEvent::RunStarted {
                    run_id: r,
                    task: "x".into(),
                    ts: chrono::Utc::now(),
                },
                "run started: x",
            ),
            (
                AgentEvent::PhaseStarted {
                    run_id: r,
                    phase_id: 1,
                    label: "work".into(),
                    planned: 3,
                    parent_span_id: None,
                    description: None,
                    role: None,
                },
                "phase 1 started",
            ),
            (
                AgentEvent::AgentStarted {
                    run_id: r,
                    phase_id: 0,
                    agent_id: r,
                    prompt_preview: "".into(),
                    model: None,
                    description: None,
                    role: None,
                    name: None,
                    agent_seq: 0,
                },
                "agent ",
            ),
            (
                AgentEvent::AgentStarted {
                    run_id: r,
                    phase_id: 0,
                    agent_id: r,
                    prompt_preview: "".into(),
                    model: Some("claude".into()),
                    description: None,
                    role: None,
                    name: None,
                    agent_seq: 0,
                },
                "model claude",
            ),
            (
                AgentEvent::AgentStarted {
                    run_id: r,
                    phase_id: 0,
                    agent_id: r,
                    prompt_preview: "".into(),
                    model: None,
                    description: None,
                    role: None,
                    name: Some("analyze-auth".into()),
                    agent_seq: 0,
                },
                "agent analyze-auth started",
            ),
            (
                AgentEvent::AgentStarted {
                    run_id: r,
                    phase_id: 0,
                    agent_id: r,
                    prompt_preview: "".into(),
                    model: None,
                    description: Some("审查 auth".into()),
                    role: None,
                    name: None,
                    agent_seq: 0,
                },
                "agent 审查 auth started",
            ),
            (
                AgentEvent::AgentStarted {
                    run_id: r,
                    phase_id: 0,
                    agent_id: r,
                    prompt_preview: "".into(),
                    model: None,
                    description: None,
                    role: None,
                    name: Some("analyze-auth".into()),
                    agent_seq: 0,
                },
                "agent analyze-auth started",
            ),
            (
                AgentEvent::AgentProgress {
                    run_id: r,
                    agent_id: r,
                    delta: ProgressDelta::Message {
                        text: "hello".into(),
                    },
                },
                "agent ",
            ),
            (
                AgentEvent::AcpRaw {
                    run_id: r,
                    agent_id: r,
                    kind: "plan".into(),
                    raw: serde_json::json!({}),
                },
                "acp raw: plan",
            ),
            (
                AgentEvent::AgentDone {
                    run_id: r,
                    agent_id: r,
                    status: AgentStatus::Error,
                    tokens: TokenUsage::default(),
                    elapsed_ms: 5,
                    name: None,
                    agent_seq: 0,
                    output: serde_json::Value::Null,
                    findings: Vec::new(),
                    prompt: String::new(),
                },
                "agent ",
            ),
            (
                AgentEvent::AgentDone {
                    run_id: r,
                    agent_id: r,
                    status: AgentStatus::Ok,
                    tokens: TokenUsage::default(),
                    elapsed_ms: 5,
                    name: Some("analyze-auth".into()),
                    agent_seq: 0,
                    output: serde_json::Value::Null,
                    findings: Vec::new(),
                    prompt: String::new(),
                },
                "agent analyze-auth done",
            ),
            (
                AgentEvent::PhaseDone {
                    run_id: r,
                    phase_id: 1,
                    ok: 2,
                    failed: 0,
                },
                "phase 1 done: 2 ok",
            ),
            (
                AgentEvent::RunDone {
                    run_id: r,
                    status: RunStatus::Completed,
                    total_tokens: TokenUsage::default(),
                    report: serde_json::json!(null),
                },
                "run done",
            ),
            (
                AgentEvent::Log {
                    run_id: r,
                    agent_id: None,
                    level: LogLevel::Info,
                    msg: "hello".into(),
                },
                "log [Info] hello",
            ),
            (
                AgentEvent::BudgetSet {
                    run_id: r,
                    time_limit_ms: Some(5000),
                    max_rounds: Some(10),
                },
                "budget set: time=Some(5000)ms",
            ),
            (
                AgentEvent::BudgetSet {
                    run_id: r,
                    time_limit_ms: None,
                    max_rounds: None,
                },
                "budget set: time=None",
            ),
            (
                AgentEvent::ReportEmitted {
                    run_id: r,
                    phase_id: 0,
                    report: serde_json::json!({}),
                },
                "report emitted",
            ),
            (
                AgentEvent::ParallelStarted {
                    run_id: r,
                    phase_id: 0,
                    span_id: 3,
                    count: 5,
                },
                "parallel#3 started: 5 items",
            ),
            (
                AgentEvent::ParallelDone {
                    run_id: r,
                    phase_id: 0,
                    span_id: 2,
                    ok: 3,
                    failed: 0,
                    results: serde_json::json!([]),
                    elapsed_ms: 9,
                },
                "parallel#2 done: 3 ok",
            ),
            (
                AgentEvent::WorkflowStarted {
                    run_id: r,
                    span_id: 2,
                    path: "w.lua".into(),
                    args: serde_json::json!({}),
                },
                "workflow#2 started: w.lua",
            ),
            (
                AgentEvent::WorkflowDone {
                    run_id: r,
                    span_id: 2,
                    path: "w.lua".into(),
                    report: serde_json::json!(null),
                    elapsed_ms: 4,
                    error: None,
                },
                "workflow#2 done",
            ),
            (
                AgentEvent::ConvergeStarted {
                    run_id: r,
                    phase_id: 0,
                    span_id: 1,
                    items: 3,
                    max_rounds: 5,
                },
                "converge#1 started: 3 items, max 5 rounds",
            ),
            (
                AgentEvent::ConvergeDone {
                    run_id: r,
                    phase_id: 0,
                    span_id: 1,
                    rounds: 2,
                    converged: true,
                    surviving: 3,
                    result: serde_json::json!([]),
                    elapsed_ms: 10,
                    error: Some("err".into()),
                },
                "converge#1 failed",
            ),
            (
                AgentEvent::ConvergeDone {
                    run_id: r,
                    phase_id: 0,
                    span_id: 1,
                    rounds: 2,
                    converged: true,
                    surviving: 3,
                    result: serde_json::json!([]),
                    elapsed_ms: 10,
                    error: None,
                },
                "converge#1 done: 2 rounds",
            ),
            (
                AgentEvent::PipelineStarted {
                    run_id: r,
                    total_stages: 3,
                    items: 10,
                },
                "pipeline started: 3 stages, 10 items",
            ),
            (
                AgentEvent::PipelineStageStarted {
                    run_id: r,
                    stage_index: 1,
                    label: "build".into(),
                    agents_in_stage: 2,
                },
                "pipeline stage 1 started: build (2 agents)",
            ),
            (
                AgentEvent::PipelineItemDone {
                    run_id: r,
                    stage_index: 1,
                    item_index: 0,
                    status: AgentStatus::Ok,
                    tokens: t,
                    elapsed_ms: 5,
                },
                "pipeline stage 1 item 0 done: Ok (5ms)",
            ),
            (
                AgentEvent::PipelineDone {
                    run_id: r,
                    stages_completed: 3,
                    total_ok: 8,
                    total_failed: 2,
                },
                "pipeline done: 3 stages, 8 ok, 2 failed",
            ),
        ];
        for (evt, needle) in cases {
            let line = format_event_line(&evt);
            assert!(line.contains(needle), "{line:?} should contain {needle:?}");
        }
    }

    #[test]
    fn format_delta_covers_all_variants() {
        let cases: Vec<(ProgressDelta, &str)> = vec![
            (
                ProgressDelta::Message {
                    text: "hello world".into(),
                },
                "msg: hello world",
            ),
            (
                ProgressDelta::ToolCall {
                    name: "bash".into(),
                    summary: "ls".into(),
                },
                "tool: bash ls",
            ),
            (
                ProgressDelta::FileEdit {
                    path: PathBuf::from("src/main.rs"),
                },
                "edit: src/main.rs",
            ),
            (
                ProgressDelta::Tokens {
                    usage: TokenUsage {
                        input: 10,
                        output: 5,
                        cache_read: 0,
                        cache_write: 0,
                    },
                },
                "tokens: 15",
            ),
        ];
        for (delta, needle) in cases {
            let line = format_delta(&delta);
            assert!(line.contains(needle), "{line:?} should contain {needle:?}");
        }
    }

    #[test]
    fn format_delta_truncates_long_message() {
        let long = "x".repeat(200);
        let delta = ProgressDelta::Message { text: long.clone() };
        let line = format_delta(&delta);
        assert!(line.starts_with("msg: "));
        assert!(
            line.len() < 200,
            "line should be truncated, got length {}",
            line.len()
        );
        assert!(line.ends_with('\u{2026}'));
    }

    #[test]
    fn event_logger_new_invalid_path() {
        let result = EventLogger::new(
            Some(Path::new("/nonexistent_path_12345/foo.log")),
            LogFormat::Pretty,
        );
        assert!(result.is_err());
    }

    #[test]
    fn event_logger_write_failure() {
        struct FailWriter;
        impl Write for FailWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("mock write error"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut logger = EventLogger::new_with_writer(Box::new(FailWriter), LogFormat::Pretty);
        let r = rid();
        let evt = AgentEvent::Log {
            run_id: r,
            agent_id: None,
            level: LogLevel::Info,
            msg: "test".into(),
        };
        // BufWriter buffers small writes; the inner error surfaces on flush.
        logger.write(&evt).unwrap();
        let result = logger.flush();
        assert!(result.is_err());
    }

    #[test]
    fn event_logger_flush_failure() {
        struct FailFlushWriter;
        impl Write for FailFlushWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("mock flush error"))
            }
        }
        let mut logger = EventLogger::new_with_writer(Box::new(FailFlushWriter), LogFormat::Pretty);
        let r = rid();
        let evt = AgentEvent::Log {
            run_id: r,
            agent_id: None,
            level: LogLevel::Info,
            msg: "test".into(),
        };
        logger.write(&evt).unwrap();
        let result = logger.flush();
        assert!(result.is_err());
    }

    #[test]
    fn event_logger_write_pretty_flush_buffer() {
        let r = rid();
        let dir =
            std::env::temp_dir().join(format!("maestro_evlog_pretty_{}", uuid::Uuid::now_v7()));
        let evt = AgentEvent::PipelineDone {
            run_id: r,
            stages_completed: 2,
            total_ok: 5,
            total_failed: 1,
        };
        let mut logger = EventLogger::new(Some(&dir), LogFormat::Pretty).unwrap();
        logger.write(&evt).unwrap();
        logger.flush().unwrap();
        let written = std::fs::read_to_string(&dir).unwrap();
        assert!(written.contains("pipeline done"));
        std::fs::remove_file(&dir).ok();
    }
}
