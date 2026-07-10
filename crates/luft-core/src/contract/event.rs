//! Event bus contract (§1.4) — the single observability data source.
//! Each [`AgentEvent`] is consumed by the event bus subscribers; the state store persists it.

use crate::contract::backend::AgentStatus;
use crate::contract::finding::Finding;
use crate::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Broadcast sender shared by the scheduler and every event producer.
pub type EventSender = tokio::sync::broadcast::Sender<AgentEvent>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    RunStarted {
        run_id: RunId,
        task: String,
        ts: DateTime<Utc>,
    },
    PhaseStarted {
        run_id: RunId,
        phase_id: PhaseId,
        label: String,
        planned: usize,
        #[serde(default)]
        parent_span_id: Option<u32>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        role: Option<String>,
        #[serde(default)]
        ts: DateTime<Utc>,
    },
    AgentStarted {
        run_id: RunId,
        phase_id: PhaseId,
        agent_id: AgentId,
        prompt_preview: String,
        model: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        role: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        agent_seq: u32,
        #[serde(default)]
        ts: DateTime<Utc>,
    },
    AgentProgress {
        run_id: RunId,
        agent_id: AgentId,
        delta: ProgressDelta,
    },
    /// Raw ACP `session/update` passthrough — the verbatim notification, surfaced
    /// for observability. Produced only when the ACP backend has raw events
    /// enabled. Not persisted to
    /// the journal (see `acp-raw-events.md`).
    AcpRaw {
        run_id: RunId,
        agent_id: AgentId,
        /// `SessionUpdate` discriminator (the `sessionUpdate` tag), e.g.
        /// `"agent_message_chunk"`, `"plan"` — lets consumers filter without
        /// parsing `raw`.
        kind: String,
        /// The ACP `SessionUpdate`, serialized verbatim.
        raw: serde_json::Value,
    },
    AgentDone {
        run_id: RunId,
        agent_id: AgentId,
        status: AgentStatus,
        tokens: TokenUsage,
        elapsed_ms: u64,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        agent_seq: u32,
        #[serde(default)]
        ts: DateTime<Utc>,
        #[serde(default)]
        output: serde_json::Value,
        #[serde(default)]
        findings: Vec<Finding>,
        #[serde(default)]
        prompt: String,
        #[serde(default)]
        retry_count: u32,
    },
    PhaseDone {
        run_id: RunId,
        phase_id: PhaseId,
        ok: usize,
        failed: usize,
        #[serde(default)]
        ts: DateTime<Utc>,
    },
    RunDone {
        run_id: RunId,
        status: RunStatus,
        total_tokens: TokenUsage,
        report: serde_json::Value,
        #[serde(default)]
        ts: DateTime<Utc>,
    },
    Log {
        run_id: RunId,
        agent_id: Option<AgentId>,
        level: LogLevel,
        msg: String,
    },
    // SDK primitive events (§ sdk-events.md) — DSL-granularity observability for
    // the orchestration script. Blocking primitives emit a Started/Done span
    // pair correlated by `span_id`; instantaneous ones emit a single event.
    BudgetSet {
        run_id: RunId,
        time_limit_ms: Option<u64>,
        max_rounds: Option<u32>,
    },
    ReportEmitted {
        run_id: RunId,
        phase_id: PhaseId,
        report: serde_json::Value,
    },
    ParallelStarted {
        run_id: RunId,
        phase_id: PhaseId,
        span_id: u64,
        count: usize,
    },
    ParallelDone {
        run_id: RunId,
        phase_id: PhaseId,
        span_id: u64,
        ok: usize,
        failed: usize,
        results: serde_json::Value,
        elapsed_ms: u64,
    },
    WorkflowStarted {
        run_id: RunId,
        span_id: u64,
        path: String,
        args: serde_json::Value,
    },
    WorkflowDone {
        run_id: RunId,
        span_id: u64,
        path: String,
        report: serde_json::Value,
        elapsed_ms: u64,
        error: Option<String>,
    },
    ConvergeStarted {
        run_id: RunId,
        phase_id: PhaseId,
        span_id: u64,
        items: usize,
        max_rounds: u32,
    },
    ConvergeDone {
        run_id: RunId,
        phase_id: PhaseId,
        span_id: u64,
        rounds: u32,
        converged: bool,
        surviving: usize,
        result: serde_json::Value,
        elapsed_ms: u64,
        error: Option<String>,
    },
    // M2 Pipeline events
    PipelineStarted {
        run_id: RunId,
        total_stages: usize,
        items: usize,
    },
    PipelineStageStarted {
        run_id: RunId,
        stage_index: usize,
        label: String,
        agents_in_stage: usize,
    },
    PipelineItemDone {
        run_id: RunId,
        stage_index: usize,
        item_index: usize,
        status: AgentStatus,
        tokens: TokenUsage,
        elapsed_ms: u64,
    },
    PipelineDone {
        run_id: RunId,
        stages_completed: usize,
        total_ok: usize,
        total_failed: usize,
    },
    /// Structural phase span started — emitted by `phase_begin()`.
    PhaseSpanStarted {
        run_id: RunId,
        span_id: u32,
        name: String,
        parent_id: Option<u32>,
        depth: u32,
        planned: usize,
    },
    /// Structural phase span done — emitted by `phase_end()`.
    PhaseSpanDone {
        run_id: RunId,
        span_id: u32,
        name: String,
        parent_id: Option<u32>,
        depth: u32,
        elapsed_ms: u64,
        status: String,
    },
    /// Agent output failed schema validation and is being retried with corrective
    /// feedback injected into the prompt. Consumers (CLI, event log) can use this
    /// to inform users that an extra LLM round-trip is underway.
    SchemaRetry {
        run_id: RunId,
        agent_id: AgentId,
        attempt: u32,
        max: u32,
    },
    /// Plan preview — emitted before execution starts, from the `meta` table.
    /// Lists the declared phases so the CLI can render a plan overview before
    /// real-time execution output begins.
    PlanPreview {
        run_id: RunId,
        reasoning: String,
        phases: Vec<PlanPhase>,
    },
    /// OS signal received (SIGINT / SIGTERM / Ctrl+C). `run_id` is `None` if
    /// the signal arrived before a run had started. Emitted by the process-
    /// level signal handler in [`crate::signal`].
    SignalReceived {
        run_id: Option<RunId>,
        signal: String,
        ts: DateTime<Utc>,
    },
}

/// A single phase entry in the plan preview `meta.phases` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanPhase {
    pub label: String,
    #[serde(default)]
    pub dynamic: bool,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProgressDelta {
    Message { text: String },
    ToolCall { name: String, summary: String },
    FileEdit { path: PathBuf },
    Tokens { usage: TokenUsage },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Failed,
    Cancelled,
    Partial,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::finding::{Finding, Location, Severity};
    use chrono::TimeZone;
    use serde_json::json;
    use uuid::Uuid;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 2, 3, 4, 5).unwrap()
    }

    fn run_id() -> RunId {
        Uuid::nil()
    }

    fn agent_id() -> AgentId {
        Uuid::nil()
    }

    // ── RunStatus ────────────────────────────────────────────────

    #[test]
    fn run_status_serializes_as_snake_case() {
        assert_eq!(serde_json::to_string(&RunStatus::Completed).unwrap(), "\"completed\"");
        assert_eq!(serde_json::to_string(&RunStatus::Failed).unwrap(), "\"failed\"");
        assert_eq!(serde_json::to_string(&RunStatus::Cancelled).unwrap(), "\"cancelled\"");
        assert_eq!(serde_json::to_string(&RunStatus::Partial).unwrap(), "\"partial\"");
    }

    #[test]
    fn run_status_deserializes_from_snake_case() {
        assert_eq!(
            serde_json::from_str::<RunStatus>("\"completed\"").unwrap(),
            RunStatus::Completed
        );
        assert_eq!(
            serde_json::from_str::<RunStatus>("\"failed\"").unwrap(),
            RunStatus::Failed
        );
        assert_eq!(
            serde_json::from_str::<RunStatus>("\"cancelled\"").unwrap(),
            RunStatus::Cancelled
        );
        assert_eq!(
            serde_json::from_str::<RunStatus>("\"partial\"").unwrap(),
            RunStatus::Partial
        );
    }

    #[test]
    fn run_status_equality_and_copy() {
        let a = RunStatus::Completed;
        let b = a; // Copy semantics
        let c = a;
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn run_status_unknown_variant_fails() {
        let r: Result<RunStatus, _> = serde_json::from_str("\"unknown\"");
        assert!(r.is_err());
    }

    // ── LogLevel ─────────────────────────────────────────────────

    #[test]
    fn log_level_serializes_as_lowercase() {
        assert_eq!(serde_json::to_string(&LogLevel::Trace).unwrap(), "\"trace\"");
        assert_eq!(serde_json::to_string(&LogLevel::Debug).unwrap(), "\"debug\"");
        assert_eq!(serde_json::to_string(&LogLevel::Info).unwrap(), "\"info\"");
        assert_eq!(serde_json::to_string(&LogLevel::Warn).unwrap(), "\"warn\"");
        assert_eq!(serde_json::to_string(&LogLevel::Error).unwrap(), "\"error\"");
    }

    #[test]
    fn log_level_deserializes_from_lowercase() {
        assert_eq!(
            serde_json::from_str::<LogLevel>("\"warn\"").unwrap(),
            LogLevel::Warn
        );
        assert_eq!(
            serde_json::from_str::<LogLevel>("\"error\"").unwrap(),
            LogLevel::Error
        );
    }

    #[test]
    fn log_level_equality_and_copy() {
        let a = LogLevel::Info;
        let b = a;
        assert_eq!(a, b);
    }

    // ── ProgressDelta ────────────────────────────────────────────

    #[test]
    fn progress_delta_message_roundtrip() {
        let d = ProgressDelta::Message {
            text: "hello".into(),
        };
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("\"kind\":\"message\""));
        let back: ProgressDelta = serde_json::from_str(&s).unwrap();
        match back {
            ProgressDelta::Message { text } => assert_eq!(text, "hello"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn progress_delta_tool_call_roundtrip() {
        let d = ProgressDelta::ToolCall {
            name: "bash".into(),
            summary: "run ls".into(),
        };
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("\"kind\":\"tool_call\""));
        let back: ProgressDelta = serde_json::from_str(&s).unwrap();
        match back {
            ProgressDelta::ToolCall { name, summary } => {
                assert_eq!(name, "bash");
                assert_eq!(summary, "run ls");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn progress_delta_file_edit_roundtrip() {
        let d = ProgressDelta::FileEdit {
            path: PathBuf::from("src/main.rs"),
        };
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("\"kind\":\"file_edit\""));
        let back: ProgressDelta = serde_json::from_str(&s).unwrap();
        match back {
            ProgressDelta::FileEdit { path } => assert_eq!(path, PathBuf::from("src/main.rs")),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn progress_delta_tokens_roundtrip() {
        let d = ProgressDelta::Tokens {
            usage: TokenUsage {
                input: 10,
                output: 20,
                cache_read: 1,
                cache_write: 2,
            },
        };
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("\"kind\":\"tokens\""));
        let back: ProgressDelta = serde_json::from_str(&s).unwrap();
        match back {
            ProgressDelta::Tokens { usage } => {
                assert_eq!(usage.input, 10);
                assert_eq!(usage.output, 20);
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── PlanPhase ────────────────────────────────────────────────

    #[test]
    fn plan_phase_minimal_roundtrip() {
        let raw = json!({"label": "review"});
        let p: PlanPhase = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(p.label, "review");
        assert!(!p.dynamic);
        assert!(p.description.is_none());
        let back = serde_json::to_value(&p).unwrap();
        assert_eq!(back, json!({"label": "review", "dynamic": false, "description": null}));
    }

    #[test]
    fn plan_phase_full_roundtrip() {
        let p = PlanPhase {
            label: "implement".into(),
            dynamic: true,
            description: Some("build the thing".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: PlanPhase = serde_json::from_str(&s).unwrap();
        assert_eq!(back.label, "implement");
        assert!(back.dynamic);
        assert_eq!(back.description.as_deref(), Some("build the thing"));
    }

    // ── AgentEvent variants ──────────────────────────────────────

    #[test]
    fn event_run_started_roundtrip() {
        let ev = AgentEvent::RunStarted {
            run_id: run_id(),
            task: "investigate".into(),
            ts: ts(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"run_started\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::RunStarted { task, .. } => assert_eq!(task, "investigate"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_phase_started_minimal_roundtrip() {
        let ev = AgentEvent::PhaseStarted {
            run_id: run_id(),
            phase_id: 3,
            label: "scan".into(),
            planned: 5,
            parent_span_id: None,
            description: None,
            role: None,
            ts: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"phase_started\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::PhaseStarted {
                phase_id,
                label,
                planned,
                ..
            } => {
                assert_eq!(phase_id, 3);
                assert_eq!(label, "scan");
                assert_eq!(planned, 5);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_phase_started_omits_optional_fields() {
        let ev = AgentEvent::PhaseStarted {
            run_id: run_id(),
            phase_id: 0,
            label: "x".into(),
            planned: 0,
            parent_span_id: None,
            description: None,
            role: None,
            ts: ts(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        // Optional fields with #[serde(default)] deserialize when omitted but
        // serialize as JSON null when None. Verify they can be omitted on the
        // input side by deserializing a payload that drops them.
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::PhaseStarted {
                parent_span_id,
                description,
                role,
                ..
            } => {
                assert!(parent_span_id.is_none());
                assert!(description.is_none());
                assert!(role.is_none());
            }
            _ => panic!("wrong variant"),
        }
        assert!(s.contains("\"parent_span_id\":null"));
        assert!(s.contains("\"description\":null"));
        assert!(s.contains("\"role\":null"));
    }

    #[test]
    fn event_agent_started_roundtrip() {
        let ev = AgentEvent::AgentStarted {
            run_id: run_id(),
            phase_id: 1,
            agent_id: agent_id(),
            prompt_preview: "find bugs".into(),
            model: Some("gpt-4".into()),
            description: Some("audit".into()),
            role: Some("reviewer".into()),
            name: Some("agent-1".into()),
            agent_seq: 7,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"agent_started\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::AgentStarted {
                agent_seq,
                model,
                name,
                ..
            } => {
                assert_eq!(agent_seq, 7);
                assert_eq!(model.as_deref(), Some("gpt-4"));
                assert_eq!(name.as_deref(), Some("agent-1"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_agent_progress_roundtrip() {
        let ev = AgentEvent::AgentProgress {
            run_id: run_id(),
            agent_id: agent_id(),
            delta: ProgressDelta::Message {
                text: "ok".into(),
            },
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"agent_progress\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::AgentProgress { delta, .. } => match delta {
                ProgressDelta::Message { text } => assert_eq!(text, "ok"),
                _ => panic!("wrong delta"),
            },
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_acp_raw_roundtrip() {
        let ev = AgentEvent::AcpRaw {
            run_id: run_id(),
            agent_id: agent_id(),
            kind: "agent_message_chunk".into(),
            raw: json!({"chunk": "hi"}),
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"acp_raw\""));
        assert!(s.contains("\"kind\":\"agent_message_chunk\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::AcpRaw { kind, raw, .. } => {
                assert_eq!(kind, "agent_message_chunk");
                assert_eq!(raw, json!({"chunk": "hi"}));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_agent_done_minimal_roundtrip() {
        // Required fields only; optional fields should default.
        let raw = json!({
            "type": "agent_done",
            "run_id": run_id(),
            "agent_id": agent_id(),
            "status": "Ok",
            "tokens": {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0},
            "elapsed_ms": 12,
        });
        let ev: AgentEvent = serde_json::from_value(raw).unwrap();
        match ev {
            AgentEvent::AgentDone {
                status,
                elapsed_ms,
                name,
                agent_seq,
                output,
                findings,
                prompt,
                retry_count,
                ..
            } => {
                assert_eq!(status, AgentStatus::Ok);
                assert_eq!(elapsed_ms, 12);
                assert!(name.is_none());
                assert_eq!(agent_seq, 0);
                assert_eq!(output, serde_json::Value::Null);
                assert!(findings.is_empty());
                assert_eq!(prompt, "");
                assert_eq!(retry_count, 0);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_agent_done_full_roundtrip() {
        let finding = Finding {
            kind: "x".into(),
            severity: Severity::Low,
            title: "t".into(),
            detail: "d".into(),
            location: Some(Location {
                file: PathBuf::from("f.rs"),
                line: Some(1),
            }),
            evidence: vec!["e1".into()],
            data: json!({"k": 1}),
        };
        let ev = AgentEvent::AgentDone {
            run_id: run_id(),
            agent_id: agent_id(),
            status: AgentStatus::Ok,
            tokens: TokenUsage {
                input: 1,
                output: 2,
                cache_read: 0,
                cache_write: 0,
            },
            elapsed_ms: 42,
            name: Some("agent-A".into()),
            agent_seq: 3,
            output: json!({"answer": "yes"}),
            findings: vec![finding],
            prompt: "do it".into(),
            retry_count: 1,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"agent_done\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::AgentDone {
                status,
                elapsed_ms,
                name,
                agent_seq,
                output,
                findings,
                prompt,
                retry_count,
                ..
            } => {
                assert_eq!(status, AgentStatus::Ok);
                assert_eq!(elapsed_ms, 42);
                assert_eq!(name.as_deref(), Some("agent-A"));
                assert_eq!(agent_seq, 3);
                assert_eq!(output, json!({"answer": "yes"}));
                assert_eq!(findings.len(), 1);
                assert_eq!(prompt, "do it");
                assert_eq!(retry_count, 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_phase_done_minimal_roundtrip() {
        let raw = json!({
            "type": "phase_done",
            "run_id": run_id(),
            "phase_id": 0,
            "ok": 4,
            "failed": 1,
        });
        let ev: AgentEvent = serde_json::from_value(raw).unwrap();
        match ev {
            AgentEvent::PhaseDone { ok, failed, ts, .. } => {
                assert_eq!(ok, 4);
                assert_eq!(failed, 1);
                // Default timestamp on the optional field
                let _ = ts;
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_run_done_roundtrip() {
        let ev = AgentEvent::RunDone {
            run_id: run_id(),
            status: RunStatus::Completed,
            total_tokens: TokenUsage {
                input: 100,
                output: 50,
                cache_read: 0,
                cache_write: 0,
            },
            report: json!({"summary": "ok"}),
            ts: ts(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"run_done\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::RunDone { status, report, .. } => {
                assert_eq!(status, RunStatus::Completed);
                assert_eq!(report, json!({"summary": "ok"}));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_log_roundtrip() {
        let ev = AgentEvent::Log {
            run_id: run_id(),
            agent_id: None,
            level: LogLevel::Warn,
            msg: "watch out".into(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"log\""));
        assert!(s.contains("\"level\":\"warn\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::Log { level, msg, .. } => {
                assert_eq!(level, LogLevel::Warn);
                assert_eq!(msg, "watch out");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_budget_set_roundtrip() {
        let ev = AgentEvent::BudgetSet {
            run_id: run_id(),
            time_limit_ms: Some(60_000),
            max_rounds: Some(10),
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"budget_set\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::BudgetSet {
                time_limit_ms,
                max_rounds,
                ..
            } => {
                assert_eq!(time_limit_ms, Some(60_000));
                assert_eq!(max_rounds, Some(10));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_report_emitted_roundtrip() {
        let ev = AgentEvent::ReportEmitted {
            run_id: run_id(),
            phase_id: 2,
            report: json!({"x": 1}),
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"report_emitted\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::ReportEmitted { phase_id, report, .. } => {
                assert_eq!(phase_id, 2);
                assert_eq!(report, json!({"x": 1}));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_parallel_started_roundtrip() {
        let ev = AgentEvent::ParallelStarted {
            run_id: run_id(),
            phase_id: 4,
            span_id: 99,
            count: 8,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"parallel_started\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::ParallelStarted { span_id, count, .. } => {
                assert_eq!(span_id, 99);
                assert_eq!(count, 8);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_parallel_done_roundtrip() {
        let ev = AgentEvent::ParallelDone {
            run_id: run_id(),
            phase_id: 4,
            span_id: 99,
            ok: 3,
            failed: 1,
            results: json!([{"ok": true}, {"ok": false}]),
            elapsed_ms: 250,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"parallel_done\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::ParallelDone {
                ok,
                failed,
                elapsed_ms,
                ..
            } => {
                assert_eq!(ok, 3);
                assert_eq!(failed, 1);
                assert_eq!(elapsed_ms, 250);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_workflow_started_roundtrip() {
        let ev = AgentEvent::WorkflowStarted {
            run_id: run_id(),
            span_id: 1,
            path: "/tmp/wf.lua".into(),
            args: json!({"x": 1}),
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"workflow_started\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::WorkflowStarted { path, args, .. } => {
                assert_eq!(path, "/tmp/wf.lua");
                assert_eq!(args, json!({"x": 1}));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_workflow_done_with_error_roundtrip() {
        let ev = AgentEvent::WorkflowDone {
            run_id: run_id(),
            span_id: 1,
            path: "/tmp/wf.lua".into(),
            report: json!({"items": 7}),
            elapsed_ms: 1000,
            error: Some("boom".into()),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::WorkflowDone { error, .. } => assert_eq!(error.as_deref(), Some("boom")),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_converge_started_roundtrip() {
        let ev = AgentEvent::ConvergeStarted {
            run_id: run_id(),
            phase_id: 2,
            span_id: 5,
            items: 12,
            max_rounds: 3,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"converge_started\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::ConvergeStarted { items, max_rounds, .. } => {
                assert_eq!(items, 12);
                assert_eq!(max_rounds, 3);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_converge_done_roundtrip() {
        let ev = AgentEvent::ConvergeDone {
            run_id: run_id(),
            phase_id: 2,
            span_id: 5,
            rounds: 4,
            converged: true,
            surviving: 2,
            result: json!({"winner": "a"}),
            elapsed_ms: 800,
            error: None,
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::ConvergeDone {
                rounds,
                converged,
                surviving,
                error,
                ..
            } => {
                assert_eq!(rounds, 4);
                assert!(converged);
                assert_eq!(surviving, 2);
                assert!(error.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_pipeline_started_roundtrip() {
        let ev = AgentEvent::PipelineStarted {
            run_id: run_id(),
            total_stages: 4,
            items: 10,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"pipeline_started\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::PipelineStarted {
                run_id: _,
                total_stages,
                items,
            } => {
                assert_eq!(total_stages, 4);
                assert_eq!(items, 10);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_pipeline_stage_started_roundtrip() {
        let ev = AgentEvent::PipelineStageStarted {
            run_id: run_id(),
            stage_index: 1,
            label: "scan".into(),
            agents_in_stage: 5,
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::PipelineStageStarted { label, .. } => assert_eq!(label, "scan"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_pipeline_item_done_roundtrip() {
        let ev = AgentEvent::PipelineItemDone {
            run_id: run_id(),
            stage_index: 0,
            item_index: 3,
            status: AgentStatus::Ok,
            tokens: TokenUsage::default(),
            elapsed_ms: 50,
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::PipelineItemDone { item_index, .. } => assert_eq!(item_index, 3),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_pipeline_done_roundtrip() {
        let ev = AgentEvent::PipelineDone {
            run_id: run_id(),
            stages_completed: 4,
            total_ok: 12,
            total_failed: 1,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"pipeline_done\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::PipelineDone {
                run_id: _,
                stages_completed,
                total_ok,
                total_failed,
            } => {
                assert_eq!(stages_completed, 4);
                assert_eq!(total_ok, 12);
                assert_eq!(total_failed, 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_phase_span_started_roundtrip() {
        let ev = AgentEvent::PhaseSpanStarted {
            run_id: run_id(),
            span_id: 11,
            name: "investigate".into(),
            parent_id: Some(1),
            depth: 2,
            planned: 4,
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::PhaseSpanStarted {
                span_id,
                depth,
                planned,
                ..
            } => {
                assert_eq!(span_id, 11);
                assert_eq!(depth, 2);
                assert_eq!(planned, 4);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_phase_span_done_roundtrip() {
        let ev = AgentEvent::PhaseSpanDone {
            run_id: run_id(),
            span_id: 11,
            name: "investigate".into(),
            parent_id: None,
            depth: 0,
            elapsed_ms: 500,
            status: "ok".into(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::PhaseSpanDone {
                status,
                elapsed_ms,
                ..
            } => {
                assert_eq!(status, "ok");
                assert_eq!(elapsed_ms, 500);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_schema_retry_roundtrip() {
        let ev = AgentEvent::SchemaRetry {
            run_id: run_id(),
            agent_id: agent_id(),
            attempt: 2,
            max: 3,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"schema_retry\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::SchemaRetry { attempt, max, .. } => {
                assert_eq!(attempt, 2);
                assert_eq!(max, 3);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_plan_preview_roundtrip() {
        let ev = AgentEvent::PlanPreview {
            run_id: run_id(),
            reasoning: "because".into(),
            phases: vec![
                PlanPhase {
                    label: "scan".into(),
                    dynamic: false,
                    description: None,
                },
                PlanPhase {
                    label: "fix".into(),
                    dynamic: true,
                    description: Some("apply patches".into()),
                },
            ],
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"plan_preview\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::PlanPreview { phases, .. } => assert_eq!(phases.len(), 2),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_signal_received_roundtrip() {
        let ev = AgentEvent::SignalReceived {
            run_id: None,
            signal: "SIGINT".into(),
            ts: ts(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"signal_received\""));
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::SignalReceived { signal, run_id, .. } => {
                assert_eq!(signal, "SIGINT");
                assert!(run_id.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_clone_preserves_variant() {
        let ev = AgentEvent::RunStarted {
            run_id: run_id(),
            task: "t".into(),
            ts: ts(),
        };
        let cloned = ev.clone();
        let s1 = serde_json::to_string(&ev).unwrap();
        let s2 = serde_json::to_string(&cloned).unwrap();
        assert_eq!(s1, s2);
    }

    #[test]
    fn event_debug_includes_type_tag() {
        let ev = AgentEvent::RunStarted {
            run_id: run_id(),
            task: "t".into(),
            ts: ts(),
        };
        let dbg = format!("{:?}", ev);
        assert!(dbg.contains("RunStarted"));
    }

    #[test]
    fn event_unknown_type_fails() {
        let raw = json!({"type": "totally_made_up", "x": 1});
        let r: Result<AgentEvent, _> = serde_json::from_value(raw);
        assert!(r.is_err());
    }

    // ── EventSender alias ────────────────────────────────────────

    #[tokio::test]
    async fn event_sender_alias_is_broadcast_sender() {
        // Compile-time check: EventSender is a broadcast::Sender<AgentEvent>.
        let (tx, _rx) = tokio::sync::broadcast::channel::<AgentEvent>(4);
        let _alias: EventSender = tx;
    }
}
