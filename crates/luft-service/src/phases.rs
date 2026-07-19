//! `service::phases` — build the human-readable phase progress view for the
//! `luft phases` subcommand.
//!
//! Two data sources are supported:
//!
//! 1. **Meta (preferred)** — the `meta = {...}` table captured at run-start
//!    and persisted in `RunCheckpoint::workflow_meta`. Provides phase
//!    labels, dependencies, and agent counts without any event parsing.
//! 2. **Events fallback** — when no meta is available (legacy scripts, runs
//!    persisted before meta was introduced), derive phase structure from the
//!    `PhaseStarted` events in `events.jsonl`.
//!
//! Agent rows come from two sources merged together:
//! - **Completed agents**: `checkpoint.agent_results` (authoritative).
//! - **Running agents**: events with `AgentStarted` but no paired `AgentDone`.

use chrono::{DateTime, Utc};
use luft_core::contract::event::AgentEvent;
use luft_core::contract::ids::RunId;
use luft_core::state::{CheckpointStatus, RunCheckpoint};
use luft_planner::PlanMeta;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Where a [`PhasesView`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhasesSource {
    Meta,
    EventsFallback,
}

/// Per-phase display status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

impl PhaseStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PhaseStatus::Pending => "pending",
            PhaseStatus::Running => "running",
            PhaseStatus::Completed => "completed",
            PhaseStatus::Failed => "failed",
        }
    }

    pub fn bracket(self) -> &'static str {
        match self {
            PhaseStatus::Pending => "[pending]",
            PhaseStatus::Running => "[running]",
            PhaseStatus::Completed => "[completed]",
            PhaseStatus::Failed => "[failed]",
        }
    }
}

/// Top-level phases view: run header + phase tree.
#[derive(Debug, Clone, Serialize)]
pub struct PhasesView {
    pub run: RunHeader,
    pub source: PhasesSource,
    pub phases: Vec<PhaseRow>,
}

/// Run-level summary info (the header line in CLI output).
#[derive(Debug, Clone, Serialize)]
pub struct RunHeader {
    pub run_id: RunId,
    pub task: String,
    pub status: CheckpointStatus,
    pub current_phase: u32,
    pub total_phases: u32,
    pub total_tokens: u64,
    pub elapsed_secs: Option<f64>,
    pub created_at: u64,
}

/// One phase row (with mounted agent sub-rows).
#[derive(Debug, Clone, Serialize)]
pub struct PhaseRow {
    pub phase_id: u32,
    pub label: String,
    pub detail: Option<String>,
    pub status: PhaseStatus,
    pub planned: Option<usize>,
    pub ok: usize,
    pub failed: usize,
    pub elapsed_secs: Option<f64>,
    pub agents: Vec<AgentRow>,
}

/// Agent sub-row (mounted under a phase).
#[derive(Debug, Clone, Serialize)]
pub struct AgentRow {
    pub short_id: String,
    pub status: String,
    pub tokens: Option<u64>,
    pub findings: usize,
    pub tool_count: Option<usize>,
    pub last_message: Option<String>,
}

/// Build the phases view from a checkpoint + events.
///
/// Pure function: no I/O. Callers read files and pass the data in.
pub fn build_phases_view(checkpoint: &RunCheckpoint, events: &[AgentEvent]) -> PhasesView {
    let source = if checkpoint.workflow_meta.is_some() {
        PhasesSource::Meta
    } else {
        PhasesSource::EventsFallback
    };

    let phases = match &checkpoint.workflow_meta {
        Some(meta_json) => {
            let meta: luft_planner::PlanMeta = serde_json::from_value(meta_json.clone())
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "failed to deserialize workflow_meta; falling back to events");
                    luft_planner::PlanMeta::default()
                });
            build_from_meta(&meta, checkpoint, events)
        }
        None => build_from_events(checkpoint, events),
    };

    let total_phases = phases.len() as u32;
    let elapsed_secs = compute_run_elapsed(events, checkpoint);

    PhasesView {
        run: RunHeader {
            run_id: checkpoint.run_id,
            task: checkpoint.task.clone(),
            status: checkpoint.status.clone(),
            current_phase: checkpoint.current_phase,
            total_phases,
            total_tokens: checkpoint.total_tokens,
            elapsed_secs,
            created_at: checkpoint.created_at,
        },
        source,
        phases,
    }
}

// ---------------------------------------------------------------------------
// Phase list construction
// ---------------------------------------------------------------------------

fn build_from_meta(
    meta: &PlanMeta,
    checkpoint: &RunCheckpoint,
    events: &[AgentEvent],
) -> Vec<PhaseRow> {
    let phase_started_ts = collect_phase_started_ts(events);
    let phase_done_info = collect_phase_done_info(events);
    let completed_map = build_completed_map(checkpoint);
    let completed_agents = collect_completed_agents(checkpoint);
    let running_agents = collect_running_agents(events, &checkpoint.agent_results);

    meta.phases
        .iter()
        .enumerate()
        .map(|(idx, mp)| {
            let phase_id = (idx + 1) as u32;

            let (ok, failed, status) = resolve_phase_status(phase_id, checkpoint, &completed_map);

            let elapsed_secs = compute_phase_elapsed(phase_id, &phase_started_ts, &phase_done_info);

            let agents = build_agent_rows(phase_id, &completed_agents, &running_agents);

            PhaseRow {
                phase_id,
                label: mp.label.clone(),
                detail: Some(mp.detail.clone()).filter(|d| !d.is_empty()),
                status,
                planned: if mp.agents > 0 { Some(mp.agents) } else { None },
                ok,
                failed,
                elapsed_secs,
                agents,
            }
        })
        .collect()
}

fn build_from_events(checkpoint: &RunCheckpoint, events: &[AgentEvent]) -> Vec<PhaseRow> {
    let phase_started_ts = collect_phase_started_ts(events);
    let phase_done_info = collect_phase_done_info(events);
    let completed_map = build_completed_map(checkpoint);
    let completed_agents = collect_completed_agents(checkpoint);
    let running_agents = collect_running_agents(events, &checkpoint.agent_results);

    // Collect phase ids from events (PhaseStarted), deduped and sorted.
    let mut phase_ids: Vec<(u32, String, usize)> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    for e in events {
        if let AgentEvent::PhaseStarted {
            phase_id,
            label,
            planned,
            ..
        } = e
        {
            if seen.insert(*phase_id) {
                phase_ids.push((*phase_id, label.clone(), *planned));
            }
        }
    }

    if phase_ids.is_empty() {
        return vec![];
    }

    phase_ids
        .iter()
        .map(|(phase_id, label, planned)| {
            let (ok, failed, status) = resolve_phase_status(*phase_id, checkpoint, &completed_map);
            let elapsed_secs =
                compute_phase_elapsed(*phase_id, &phase_started_ts, &phase_done_info);
            let agents = build_agent_rows(*phase_id, &completed_agents, &running_agents);

            PhaseRow {
                phase_id: *phase_id,
                label: label.clone(),
                detail: None,
                status,
                planned: Some(*planned),
                ok,
                failed,
                elapsed_secs,
                agents,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Status / ok / failed resolution
// ---------------------------------------------------------------------------

fn resolve_phase_status(
    phase_id: u32,
    checkpoint: &RunCheckpoint,
    completed_map: &HashMap<u32, (usize, usize)>,
) -> (usize, usize, PhaseStatus) {
    if let Some((ok, failed)) = completed_map.get(&phase_id) {
        let status = if *failed > 0 {
            PhaseStatus::Failed
        } else {
            PhaseStatus::Completed
        };
        return (*ok, *failed, status);
    }

    // Not in completed_phases — infer from current_phase.
    let status = if checkpoint.current_phase == 0 {
        PhaseStatus::Pending
    } else if phase_id < checkpoint.current_phase {
        PhaseStatus::Completed
    } else if phase_id == checkpoint.current_phase {
        PhaseStatus::Running
    } else {
        PhaseStatus::Pending
    };

    (0, 0, status)
}

fn build_completed_map(checkpoint: &RunCheckpoint) -> HashMap<u32, (usize, usize)> {
    checkpoint
        .completed_phases
        .iter()
        .map(|s| (s.phase_id, (s.ok, s.failed)))
        .collect()
}

// ---------------------------------------------------------------------------
// Agent row construction
// ---------------------------------------------------------------------------

/// Collect agents from checkpoint.agent_results, grouped by phase_id.
fn collect_completed_agents(checkpoint: &RunCheckpoint) -> HashMap<u32, Vec<AgentRow>> {
    let mut map: HashMap<u32, Vec<AgentRow>> = HashMap::new();
    for cache in checkpoint.agent_results.values() {
        let row = AgentRow {
            short_id: format!("{:.8}", cache.agent_id),
            status: agent_status_str(&cache.status),
            tokens: Some(cache.tokens),
            findings: cache.findings.len(),
            tool_count: None,
            last_message: None,
        };
        map.entry(cache.phase_id).or_default().push(row);
    }
    map
}

/// Collect running agents: AgentStarted without a paired AgentDone.
fn collect_running_agents(
    events: &[AgentEvent],
    completed: &HashMap<luft_core::contract::ids::AgentId, luft_core::state::AgentResultCache>,
) -> HashMap<u32, Vec<AgentRow>> {
    let mut done_agents: HashSet<luft_core::contract::ids::AgentId> = HashSet::new();
    for e in events {
        if let AgentEvent::AgentDone { agent_id, .. } = e {
            done_agents.insert(*agent_id);
        }
    }

    let mut map: HashMap<u32, Vec<AgentRow>> = HashMap::new();
    for e in events {
        if let AgentEvent::AgentStarted {
            phase_id, agent_id, ..
        } = e
        {
            if completed.contains_key(agent_id) || done_agents.contains(agent_id) {
                continue;
            }
            let row = AgentRow {
                short_id: format!("{:.8}", agent_id),
                status: "running".to_string(),
                tokens: None,
                findings: 0,
                tool_count: None,
                last_message: None,
            };
            map.entry(*phase_id).or_default().push(row);
        }
    }
    map
}

fn build_agent_rows(
    phase_id: u32,
    completed: &HashMap<u32, Vec<AgentRow>>,
    running: &HashMap<u32, Vec<AgentRow>>,
) -> Vec<AgentRow> {
    let mut agents = Vec::new();
    if let Some(c) = completed.get(&phase_id) {
        agents.extend(c.iter().cloned());
    }
    if let Some(r) = running.get(&phase_id) {
        agents.extend(r.iter().cloned());
    }
    agents
}

fn agent_status_str(status: &str) -> String {
    match status {
        "ok" | "Ok" | "OK" => "completed".to_string(),
        "error" | "Error" => "failed".to_string(),
        "cancelled" | "Cancelled" => "cancelled".to_string(),
        "timed_out" | "TimedOut" | "timedout" => "timed_out".to_string(),
        other => other.to_lowercase(),
    }
}

// ---------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------

fn collect_phase_started_ts(events: &[AgentEvent]) -> HashMap<u32, DateTime<Utc>> {
    let mut map = HashMap::new();
    for e in events {
        if let AgentEvent::PhaseStarted { phase_id, ts, .. } = e {
            map.entry(*phase_id).or_insert(*ts);
        }
    }
    map
}

#[derive(Debug)]
struct PhaseDoneInfo {
    ts: DateTime<Utc>,
    #[allow(dead_code)]
    ok: usize,
    #[allow(dead_code)]
    failed: usize,
}

fn collect_phase_done_info(events: &[AgentEvent]) -> HashMap<u32, PhaseDoneInfo> {
    let mut map = HashMap::new();
    for e in events {
        if let AgentEvent::PhaseDone {
            phase_id,
            ts,
            ok,
            failed,
            ..
        } = e
        {
            map.insert(
                *phase_id,
                PhaseDoneInfo {
                    ts: *ts,
                    ok: *ok,
                    failed: *failed,
                },
            );
        }
    }
    map
}

fn compute_phase_elapsed(
    phase_id: u32,
    started: &HashMap<u32, DateTime<Utc>>,
    done: &HashMap<u32, PhaseDoneInfo>,
) -> Option<f64> {
    let start = started.get(&phase_id)?;
    let end = done.get(&phase_id)?;
    let dur = end.ts.signed_duration_since(*start);
    let secs = dur.num_milliseconds() as f64 / 1000.0;
    if secs >= 0.0 {
        Some(secs)
    } else {
        None
    }
}

fn compute_run_elapsed(events: &[AgentEvent], checkpoint: &RunCheckpoint) -> Option<f64> {
    let run_started = events.iter().find_map(|e| {
        if let AgentEvent::RunStarted { ts, .. } = e {
            Some(*ts)
        } else {
            None
        }
    });

    let run_done = events.iter().find_map(|e| {
        if let AgentEvent::RunDone { ts, .. } = e {
            Some(*ts)
        } else {
            None
        }
    });

    match (run_started, run_done) {
        (Some(start), Some(end)) => {
            let secs = end.signed_duration_since(start).num_milliseconds() as f64 / 1000.0;
            if secs >= 0.0 {
                Some(secs)
            } else {
                None
            }
        }
        (Some(start), None) => {
            // Run still in progress — use now.
            let secs = Utc::now().signed_duration_since(start).num_milliseconds() as f64 / 1000.0;
            if secs >= 0.0 {
                Some(secs)
            } else {
                None
            }
        }
        (None, _) => {
            // No RunStarted event — fallback to checkpoint timestamps.
            if checkpoint.created_at > 0 && checkpoint.updated_at > checkpoint.created_at {
                Some((checkpoint.updated_at - checkpoint.created_at) as f64)
            } else {
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use luft_core::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};
    use luft_core::state::{AgentResultCache, PhaseSummary};
    use luft_planner::{MetaPhase, PlanMeta};
    use std::collections::HashMap;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn cp(meta: Option<PlanMeta>, current_phase: u32) -> RunCheckpoint {
        RunCheckpoint {
            run_id: RunId::now_v7(),
            task: "test task".into(),
            status: CheckpointStatus::Running,
            current_phase,
            completed_phases: vec![],
            agent_results: HashMap::new(),
            findings: vec![],
            total_tokens: 0,
            created_at: 1000,
            updated_at: 2000,
            completed_spans: vec![],
            workflow_meta: meta.map(|m| serde_json::to_value(m).unwrap()),
            started_agent_ids: vec![],
        }
    }

    fn cp_with_agents(
        meta: Option<PlanMeta>,
        current_phase: u32,
        agents: Vec<(AgentId, PhaseId, &str, u64, usize)>,
    ) -> RunCheckpoint {
        let mut checkpoint = cp(meta, current_phase);
        for (agent_id, phase_id, status, tokens, findings_count) in agents {
            checkpoint.agent_results.insert(
                agent_id,
                AgentResultCache {
                    agent_id,
                    phase_id,
                    status: status.to_string(),
                    output: serde_json::Value::Null,
                    findings: (0..findings_count)
                        .map(|i| luft_core::contract::finding::Finding {
                            kind: "test".into(),
                            severity: luft_core::contract::finding::Severity::Info,
                            title: format!("finding {}", i),
                            detail: String::new(),
                            location: None,
                            evidence: vec![],
                            data: serde_json::Value::Null,
                        })
                        .collect(),
                    tokens,
                    completed_at: 0,
                    cache_key_hash: None,
                    description: None,
                    role: None,
                },
            );
            checkpoint.total_tokens += tokens;
        }
        checkpoint
    }

    // ── Meta path tests ──────────────────────────────────────────

    #[test]
    fn meta_pending_when_no_events() {
        let meta = PlanMeta {
            phases: vec![MetaPhase {
                label: "x".into(),
                detail: "do x".into(),
                agents: 2,
                ..Default::default()
            }],
            reasoning: "r".into(),
        };
        let checkpoint = cp(Some(meta), 0);
        let view = build_phases_view(&checkpoint, &[]);
        assert_eq!(view.source, PhasesSource::Meta);
        assert_eq!(view.phases.len(), 1);
        assert_eq!(view.phases[0].status, PhaseStatus::Pending);
        assert!(view.phases[0].agents.is_empty());
    }

    #[test]
    fn meta_running_current_phase() {
        let meta = PlanMeta {
            phases: vec![
                MetaPhase {
                    label: "a".into(),
                    detail: "1".into(),
                    agents: 1,
                    ..Default::default()
                },
                MetaPhase {
                    label: "b".into(),
                    detail: "2".into(),
                    agents: 1,
                    ..Default::default()
                },
                MetaPhase {
                    label: "c".into(),
                    detail: "3".into(),
                    agents: 1,
                    ..Default::default()
                },
            ],
            reasoning: String::new(),
        };
        let checkpoint = cp(Some(meta), 2);
        let view = build_phases_view(&checkpoint, &[]);
        assert_eq!(view.phases[0].status, PhaseStatus::Completed);
        assert_eq!(view.phases[1].status, PhaseStatus::Running);
        assert_eq!(view.phases[2].status, PhaseStatus::Pending);
    }

    #[test]
    fn meta_with_completed_agents() {
        let meta = PlanMeta {
            phases: vec![MetaPhase {
                label: "gather".into(),
                detail: "collect".into(),
                agents: 2,
                ..Default::default()
            }],
            reasoning: String::new(),
        };
        let a1 = AgentId::now_v7();
        let a2 = AgentId::now_v7();
        let checkpoint = cp_with_agents(
            Some(meta),
            1,
            vec![(a1, 1, "ok", 120, 0), (a2, 1, "ok", 80, 1)],
        );
        let view = build_phases_view(&checkpoint, &[]);
        assert_eq!(view.phases[0].agents.len(), 2);
        let tokens: std::collections::HashSet<_> =
            view.phases[0].agents.iter().map(|a| a.tokens).collect();
        assert!(tokens.contains(&Some(120)));
        assert!(tokens.contains(&Some(80)));
        let findings_total: usize = view.phases[0].agents.iter().map(|a| a.findings).sum();
        assert_eq!(findings_total, 1);
    }

    #[test]
    fn meta_with_running_agent_from_events() {
        let meta = PlanMeta {
            phases: vec![MetaPhase {
                label: "analyze".into(),
                detail: "analyze".into(),
                agents: 2,
                ..Default::default()
            }],
            reasoning: String::new(),
        };
        let a1 = AgentId::now_v7();
        let running_agent = AgentId::now_v7();
        let checkpoint = cp_with_agents(Some(meta), 1, vec![(a1, 1, "ok", 640, 2)]);
        let events = vec![AgentEvent::AgentStarted {
            run_id: checkpoint.run_id,
            phase_id: 1,
            agent_id: running_agent,
            prompt_preview: "working".into(),
            model: None,
            description: None,
            role: None,
            name: None,
            agent_seq: 0,
        }];
        let view = build_phases_view(&checkpoint, &events);
        assert_eq!(view.phases[0].agents.len(), 2);
        // First agent is completed (from checkpoint)
        assert_eq!(view.phases[0].agents[0].status, "completed");
        assert_eq!(view.phases[0].agents[0].tokens, Some(640));
        // Second agent is running (from events)
        assert_eq!(view.phases[0].agents[1].status, "running");
        assert_eq!(view.phases[0].agents[1].tokens, None);
        assert_eq!(view.phases[0].agents[1].tool_count, None);
    }

    #[test]
    fn meta_phase_elapsed_from_ts() {
        let meta = PlanMeta {
            phases: vec![MetaPhase {
                label: "p".into(),
                detail: "d".into(),
                agents: 1,
                ..Default::default()
            }],
            reasoning: String::new(),
        };
        let checkpoint = cp(Some(meta), 1);
        let events = vec![
            AgentEvent::PhaseStarted {
                run_id: checkpoint.run_id,
                phase_id: 1,
                label: "p".into(),
                planned: 1,
                parent_span_id: None,
                description: None,
                role: None,
                ts: ts(100),
            },
            AgentEvent::PhaseDone {
                run_id: checkpoint.run_id,
                phase_id: 1,
                ok: 1,
                failed: 0,
                ts: ts(103),
            },
        ];
        let view = build_phases_view(&checkpoint, &events);
        assert_eq!(view.phases[0].elapsed_secs, Some(3.0));
    }

    #[test]
    fn meta_phase_elapsed_none_when_ts_missing() {
        let meta = PlanMeta {
            phases: vec![MetaPhase {
                label: "p".into(),
                detail: "d".into(),
                agents: 1,
                ..Default::default()
            }],
            reasoning: String::new(),
        };
        let checkpoint = cp(Some(meta), 1);
        // No events at all → elapsed None
        let view = build_phases_view(&checkpoint, &[]);
        assert_eq!(view.phases[0].elapsed_secs, None);
    }

    #[test]
    fn meta_failed_phase_status() {
        let meta = PlanMeta {
            phases: vec![MetaPhase {
                label: "p".into(),
                detail: "d".into(),
                agents: 1,
                ..Default::default()
            }],
            reasoning: String::new(),
        };
        let mut checkpoint = cp(Some(meta), 1);
        checkpoint.completed_phases = vec![PhaseSummary {
            phase_id: 1,
            label: "p".into(),
            planned: 1,
            ok: 0,
            failed: 2,
            description: None,
            role: None,
        }];
        let view = build_phases_view(&checkpoint, &[]);
        assert_eq!(view.phases[0].status, PhaseStatus::Failed);
        assert_eq!(view.phases[0].ok, 0);
        assert_eq!(view.phases[0].failed, 2);
    }

    // ── Events fallback tests ────────────────────────────────────

    #[test]
    fn fallback_events_reconstructs_phases() {
        let checkpoint = cp(None, 1);
        let events = vec![
            AgentEvent::PhaseStarted {
                run_id: checkpoint.run_id,
                phase_id: 1,
                label: "discover".into(),
                planned: 2,
                parent_span_id: None,
                description: None,
                role: None,
                ts: ts(100),
            },
            AgentEvent::PhaseStarted {
                run_id: checkpoint.run_id,
                phase_id: 2,
                label: "report".into(),
                planned: 1,
                parent_span_id: None,
                description: None,
                role: None,
                ts: ts(200),
            },
        ];
        let view = build_phases_view(&checkpoint, &events);
        assert_eq!(view.source, PhasesSource::EventsFallback);
        assert_eq!(view.phases.len(), 2);
        assert_eq!(view.phases[0].label, "discover");
        assert_eq!(view.phases[1].label, "report");
        assert!(view.phases[0].detail.is_none());
    }

    #[test]
    fn fallback_no_events_empty_phases() {
        let checkpoint = cp(None, 0);
        let view = build_phases_view(&checkpoint, &[]);
        assert_eq!(view.source, PhasesSource::EventsFallback);
        assert!(view.phases.is_empty());
    }

    // ── Run header tests ────────────────────────────────────────

    #[test]
    fn header_fields_populated() {
        let meta = PlanMeta {
            phases: vec![MetaPhase {
                label: "x".into(),
                detail: "y".into(),
                agents: 1,
                ..Default::default()
            }],
            reasoning: String::new(),
        };
        let checkpoint = cp(Some(meta), 1);
        let view = build_phases_view(&checkpoint, &[]);
        assert_eq!(view.run.task, "test task");
        assert_eq!(view.run.current_phase, 1);
        assert_eq!(view.run.total_phases, 1);
    }

    #[test]
    fn header_elapsed_from_run_events() {
        let checkpoint = cp(None, 1);
        let events = vec![
            AgentEvent::RunStarted {
                run_id: checkpoint.run_id,
                task: "t".into(),
                ts: ts(100),
            },
            AgentEvent::RunDone {
                run_id: checkpoint.run_id,
                status: luft_core::contract::event::RunStatus::Completed,
                total_tokens: TokenUsage::default(),
                report: serde_json::Value::Null,
                ts: ts(142),
            },
        ];
        let view = build_phases_view(&checkpoint, &events);
        assert_eq!(view.run.elapsed_secs, Some(42.0));
    }

    #[test]
    fn header_elapsed_fallback_to_checkpoint() {
        let checkpoint = cp(None, 1);
        let view = build_phases_view(&checkpoint, &[]);
        // created_at=1000, updated_at=2000 → 1000 secs
        assert_eq!(view.run.elapsed_secs, Some(1000.0));
    }

    // ── Utility tests ───────────────────────────────────────────

    #[test]
    fn agent_status_str_variants() {
        assert_eq!(agent_status_str("ok"), "completed");
        assert_eq!(agent_status_str("error"), "failed");
        assert_eq!(agent_status_str("cancelled"), "cancelled");
        assert_eq!(agent_status_str("timed_out"), "timed_out");
    }

    #[test]
    fn phase_status_bracket() {
        assert_eq!(PhaseStatus::Pending.bracket(), "[pending]");
        assert_eq!(PhaseStatus::Running.bracket(), "[running]");
        assert_eq!(PhaseStatus::Completed.bracket(), "[completed]");
        assert_eq!(PhaseStatus::Failed.bracket(), "[failed]");
    }

    #[test]
    fn pending_phase_has_no_agents() {
        let meta = PlanMeta {
            phases: vec![
                MetaPhase {
                    label: "a".into(),
                    detail: "1".into(),
                    agents: 1,
                    ..Default::default()
                },
                MetaPhase {
                    label: "b".into(),
                    detail: "2".into(),
                    agents: 1,
                    ..Default::default()
                },
            ],
            reasoning: String::new(),
        };
        let checkpoint = cp(Some(meta), 0);
        let view = build_phases_view(&checkpoint, &[]);
        // Both pending, no agents
        for phase in &view.phases {
            assert_eq!(phase.status, PhaseStatus::Pending);
            assert!(phase.agents.is_empty());
        }
    }
}
