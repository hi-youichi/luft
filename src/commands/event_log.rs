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
        Ok(Self { sink: BufWriter::new(inner), format })
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
        AgentStarted { agent_id, model, .. } => {
            format!("agent {agent_id} started (model {})", model.as_deref().unwrap_or("default"))
        }
        AgentProgress { agent_id, delta, .. } => format!("agent {agent_id} · {}", format_delta(delta)),
        AcpRaw { kind, .. } => format!("acp raw: {kind}"),
        AgentDone { agent_id, status, tokens, elapsed_ms, .. } => {
            format!("agent {agent_id} done: {status:?} ({elapsed_ms}ms, {} tok)", tokens.total())
        }
        PhaseDone { phase_id, ok, failed, .. } => {
            format!("phase {phase_id} done: {ok} ok, {failed} failed")
        }
        RunDone { status, total_tokens, .. } => {
            format!("run done: {status:?} ({} tok)", total_tokens.total())
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
    }
}

fn format_delta(delta: &ProgressDelta) -> String {
    match delta {
        ProgressDelta::Message { text } => format!("msg: {}", truncate(text, 80)),
        ProgressDelta::ToolCall { name, summary } => format!("tool: {name} {summary}"),
        ProgressDelta::FileEdit { path } => format!("edit: {}", path.display()),
        ProgressDelta::Tokens { usage } => format!("tokens: {}", usage.total()),
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
    use maestro::core::contract::event::RunStatus;
    use maestro::core::contract::ids::{RunId, TokenUsage};

    fn rid() -> RunId {
        RunId::now_v7()
    }

    #[test]
    fn format_covers_representative_variants() {
        let r = rid();
        let cases: Vec<(AgentEvent, &str)> = vec![
            (AgentEvent::PhaseStarted { run_id: r, phase_id: 1, label: "work".into(), planned: 3 }, "phase 1 started"),
            (AgentEvent::AgentDone { run_id: r, agent_id: r, status: AgentStatus::Error, tokens: TokenUsage::default(), elapsed_ms: 5 }, "agent"),
            (AgentEvent::AcpRaw { run_id: r, agent_id: r, kind: "plan".into(), raw: serde_json::json!({}) }, "acp raw: plan"),
            (AgentEvent::ParallelDone { run_id: r, phase_id: 0, span_id: 2, ok: 3, failed: 0, results: serde_json::json!([]), elapsed_ms: 9 }, "parallel#2 done: 3 ok"),
            (AgentEvent::WorkflowDone { run_id: r, span_id: 1, path: "w.lua".into(), report: serde_json::json!(null), elapsed_ms: 4, error: Some("boom".into()) }, "workflow#1 failed"),
            (AgentEvent::RunDone { run_id: r, status: RunStatus::Completed, total_tokens: TokenUsage::default(), report: serde_json::json!(null) }, "run done"),
        ];
        for (evt, needle) in cases {
            let line = format_event_line(&evt);
            assert!(line.contains(needle), "{line:?} should contain {needle:?}");
        }
    }

    #[test]
    fn jsonl_matches_serde_to_string() {
        let evt = AgentEvent::PhaseDone { run_id: rid(), phase_id: 2, ok: 1, failed: 0 };
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
}
