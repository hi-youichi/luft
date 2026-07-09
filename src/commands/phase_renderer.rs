//! Phase-based CLI output renderer.
//!
//! Transforms the flat [`AgentEvent`] stream into a hierarchical, human-readable
//! display with live spinners (TTY) or static lines (non-TTY).
//!
//! ## Output structure
//!
//! ```text
//! ╭─ Run: deep-research
//!
//! ├── Phase 1 · Research (3 agents)
//! │   ✓ researcher-a  1.2s  4.7k tok · 5 calls
//! │   ✓ researcher-b  0.8s  510 tok · 0 calls
//! │   ✗ researcher-c  3.1s  ERROR
//! │  Phase 1 done · 2 ok, 1 failed (3.1s)
//!
//! ├── Phase 2 · Synthesis
//! │  converge · 2 rounds → converged ✓ (2 surviving, 4.2s)
//! │  Phase 2 done · 2 ok (4.2s)
//!
//! ╰─ Run done · Completed · 4.7k tok · 12.6s
//! ```

use std::collections::HashMap;
use std::time::{Duration, Instant};

use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use maestro::core::contract::backend::AgentStatus;
use maestro::core::contract::event::{AgentEvent, ProgressDelta, RunStatus};
use maestro::core::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};

// ── Data model ────────────────────────────────────────────

struct AgentEntry {
    label: String,
    description: Option<String>,
    pb: Option<ProgressBar>,
    /// Cumulative tool-call count for the live spinner display. Updated as
    /// `ProgressDelta::ToolCall` events arrive; surfaced both in the spinner
    /// message and the agent-done summary line.
    tool_calls: u32,
    /// Most recent `TokenUsage` from `ProgressDelta::Tokens`. Cached so the
    /// spinner keeps showing the latest token total even when the next delta
    /// is a `ToolCall` (which doesn't carry token info).
    latest_tokens: Option<TokenUsage>,
    reasoning_chars: usize,
    assistant_chars: usize,
}

struct PhaseEntry {
    index: usize,
    start: Instant,
    _label: String,
    _description: Option<String>,
    agents: HashMap<AgentId, AgentEntry>,
}

// ── Renderer ──────────────────────────────────────────────

pub struct PhaseRenderer {
    tty: bool,
    mp: MultiProgress,
    run_start: Option<Instant>,
    /// Task label from `RunStarted`, kept so the header bar can be re-rendered
    /// on each timer tick without storing the formatted prefix separately.
    run_task: Option<String>,
    /// Run ID captured from `RunStarted`; shown as a short prefix in the
    /// header line.
    run_id: Option<RunId>,
    /// Dedicated header bar (TTY only). Carries the
    /// `╭─ Run: <task>  ⏱ Mm SSs` line; updated in place by `tick_elapsed()`
    /// and frozen with the final elapsed time by `on_run_done`. `None` in
    /// non-TTY mode where there is no in-place rewrite to do.
    timer_pb: Option<ProgressBar>,
    phases: HashMap<PhaseId, PhaseEntry>,
    phase_order: Vec<PhaseId>,
}

impl PhaseRenderer {
    pub fn new(tty: bool) -> Self {
        Self {
            tty,
            mp: MultiProgress::new(),
            run_start: None,
            run_task: None,
            run_id: None,
            timer_pb: None,
            phases: HashMap::new(),
            phase_order: Vec::new(),
        }
    }

    /// Dispatch an event to the appropriate handler.
    pub fn handle(&mut self, evt: &AgentEvent) {
        match evt {
            AgentEvent::RunStarted { run_id, task, .. } => self.on_run_started(*run_id, task),
            AgentEvent::PhaseStarted {
                phase_id,
                label,
                planned,
                description,
                ..
            } => {
                self.on_phase_started(*phase_id, label, *planned, description.as_deref());
            }
            AgentEvent::AgentStarted {
                phase_id,
                agent_id,
                name,
                description,
                role,
                model,
                ..
            } => {
                self.on_agent_started(
                    *phase_id,
                    *agent_id,
                    name.as_deref(),
                    description.as_deref(),
                    role.as_deref(),
                    model.as_deref(),
                );
            }
            AgentEvent::AgentDone {
                agent_id,
                status,
                tokens,
                elapsed_ms,
                retry_count,
                ..
            } => {
                self.on_agent_done(
                    *agent_id,
                    status.clone(),
                    *tokens,
                    *elapsed_ms,
                    *retry_count,
                );
            }
            AgentEvent::PhaseDone {
                phase_id,
                ok,
                failed,
                ..
            } => {
                self.on_phase_done(*phase_id, *ok, *failed);
            }
            AgentEvent::RunDone {
                status,
                total_tokens,
                ..
            } => {
                self.on_run_done(*status, *total_tokens);
            }
            // Structural events — one-line summaries within current phase
            AgentEvent::ParallelDone {
                span_id,
                ok,
                failed,
                elapsed_ms,
                ..
            } => {
                self.summary(&format!(
                    "parallel#{} · {} ok, {} failed ({})",
                    span_id,
                    ok,
                    failed,
                    fmt_dur(*elapsed_ms),
                ));
            }
            AgentEvent::ConvergeDone {
                span_id,
                rounds,
                converged,
                surviving,
                elapsed_ms,
                error,
                ..
            } => {
                let msg = match error {
                    Some(e) => format!(
                        "converge#{} · failed ({}): {}",
                        span_id,
                        fmt_dur(*elapsed_ms),
                        e,
                    ),
                    None => format!(
                        "converge#{} · {} rounds → {} ({} surviving, {})",
                        span_id,
                        rounds,
                        if *converged {
                            "converged ✓"
                        } else {
                            "not converged ✗"
                        },
                        surviving,
                        fmt_dur(*elapsed_ms),
                    ),
                };
                self.summary(&msg);
            }
            AgentEvent::PipelineDone {
                stages_completed,
                total_ok,
                total_failed,
                ..
            } => {
                self.summary(&format!(
                    "pipeline · {} stages, {} ok, {} failed",
                    stages_completed, total_ok, total_failed,
                ));
            }
            AgentEvent::WorkflowDone {
                span_id,
                path,
                elapsed_ms,
                error,
                ..
            } => {
                let msg = match error {
                    Some(_) => format!(
                        "workflow#{} · {} failed ({})",
                        span_id,
                        path,
                        fmt_dur(*elapsed_ms),
                    ),
                    None => format!("workflow#{} · {} ({})", span_id, path, fmt_dur(*elapsed_ms)),
                };
                self.summary(&msg);
            }
            AgentEvent::PhaseSpanDone {
                span_id,
                name,
                elapsed_ms,
                status,
                ..
            } => {
                self.summary(&format!(
                    "span#{} · {} ({}, {})",
                    span_id,
                    name,
                    status,
                    fmt_dur(*elapsed_ms)
                ));
            }
            AgentEvent::PlanPreview {
                reasoning, phases, ..
            } => {
                self.on_plan_preview(reasoning, phases);
            }
            AgentEvent::SchemaRetry {
                agent_id,
                attempt,
                max,
                ..
            } => {
                let label = self
                    .phases
                    .values()
                    .find_map(|p| p.agents.get(agent_id))
                    .map(|e| &e.label)
                    .map(|s| s.as_str())
                    .unwrap_or("?");
                self.print(&format!(
                    "│   {} · schema mismatch · retry {}/{}",
                    style(label).yellow(),
                    attempt,
                    max,
                ));
            }
            AgentEvent::AgentProgress {
                agent_id, delta, ..
            } => {
                self.on_agent_progress(agent_id, delta);
            }
            // Intentionally ignored (decision: no AcpRaw / Log / etc.)
            _ => {}
        }
    }

    // ── Event handlers ───────────────────────────────────

    fn on_run_started(&mut self, run_id: RunId, task: &str) {
        self.run_start = Some(Instant::now());
        self.run_task = Some(task.to_string());
        self.run_id = Some(run_id);
        let short_id = &run_id.to_string()[..8];

        // Print the header as a persistent line (survives in scrollback)
        // in BOTH TTY and non-TTY modes. In TTY mode we additionally keep
        // a small ProgressBar for the live ⏱ clock — that bar lives at
        // the bottom of the terminal and would NOT appear in scrollback,
        // so the run ID must go through println to be visible later.
        self.print(&format!(
            "╭─ Run: {}  {}",
            style(task).bold(),
            style(format!("#{short_id}")).dim(),
        ));

        if self.tty {
            let pb = self.mp.add(ProgressBar::new(0));
            pb.set_style(ProgressStyle::with_template("{msg}").unwrap());
            pb.set_message(format!("⏱ {}", fmt_clock(0)));
            self.timer_pb = Some(pb);
        }
        self.print("");
    }

    /// Re-render the header bar's elapsed-time suffix with the current
    /// wall-clock duration. No-op outside TTY mode or before `RunStarted`.
    /// Designed to be called from a separate tokio interval task while the
    /// printer task handles events on the same renderer via a shared mutex.
    pub fn tick_elapsed(&self) {
        if let (Some(start), Some(pb)) = (self.run_start, &self.timer_pb) {
            pb.set_message(format!(
                "⏱ {}",
                fmt_clock(start.elapsed().as_millis() as u64)
            ));
        }
    }

    fn on_plan_preview(
        &self,
        reasoning: &str,
        phases: &[maestro::core::contract::event::PlanPhase],
    ) {
        if !reasoning.is_empty() {
            self.print(&format!("│  {}", style(reasoning).dim()));
        }
        for (i, p) in phases.iter().enumerate() {
            let marker = if p.dynamic { " ◇ dynamic" } else { "" };
            let desc_part = p
                .description
                .as_deref()
                .map(|d| format!(" · {}", style(d).dim()))
                .unwrap_or_default();
            self.print(&format!(
                "│  {} {}{}{}",
                style(format!("{}.", i + 1)).dim(),
                p.label,
                desc_part,
                if p.dynamic {
                    style(marker).yellow()
                } else {
                    style("")
                },
            ));
        }
        self.print(&format!("│  {}", style("─".repeat(40)).dim()));
    }

    fn on_phase_started(
        &mut self,
        phase_id: PhaseId,
        label: &str,
        planned: usize,
        description: Option<&str>,
    ) {
        let index = self.phase_order.len() + 1;
        self.phase_order.push(phase_id);
        self.phases.insert(
            phase_id,
            PhaseEntry {
                index,
                start: Instant::now(),
                _label: label.to_string(),
                _description: description.map(str::to_string),
                agents: HashMap::new(),
            },
        );

        let count = if planned > 0 {
            format!(" ({} agents)", planned)
        } else {
            String::new()
        };
        let desc_part = description
            .map(|d| format!(" · {}", style(d).dim()))
            .unwrap_or_default();
        self.print(&format!(
            "├── {} · {}{}{}",
            style(format!("Phase {}", index)).bold(),
            label,
            desc_part,
            count,
        ));
    }

    fn on_agent_started(
        &mut self,
        phase_id: PhaseId,
        agent_id: AgentId,
        name: Option<&str>,
        description: Option<&str>,
        role: Option<&str>,
        model: Option<&str>,
    ) {
        let phase = match self.phases.get_mut(&phase_id) {
            Some(p) => p,
            None => return,
        };

        let label = name
            .map(str::to_string)
            .or_else(|| description.map(str::to_string))
            .or_else(|| role.map(str::to_string))
            .unwrap_or_else(|| short_id(&agent_id));

        // Show description as secondary detail only when name was used as label.
        let display_desc = if name.is_some() {
            description.map(str::to_string)
        } else {
            None
        };

        if self.tty {
            let pb = self.mp.add(ProgressBar::new_spinner());
            pb.set_style(
                ProgressStyle::with_template("│   {spinner} {wide_msg}")
                    .unwrap()
                    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
            );
            pb.enable_steady_tick(Duration::from_millis(80));
            let display = match (&display_desc, model) {
                (Some(d), Some(m)) => {
                    format!("{} · {} · {}", label, style(d).dim(), style(m).dim())
                }
                (Some(d), None) => format!("{} · {}", label, style(d).dim()),
                (None, Some(m)) => format!("{} · {}", label, style(m).dim()),
                (None, None) => label.clone(),
            };
            pb.set_message(display);
            phase.agents.insert(
                agent_id,
                AgentEntry {
                    label,
                    description: display_desc,
                    pb: Some(pb),
                    tool_calls: 0,
                    latest_tokens: None,
                    reasoning_chars: 0,
                    assistant_chars: 0,
                },
            );
        } else {
            phase.agents.insert(
                agent_id,
                AgentEntry {
                    label,
                    description: display_desc,
                    pb: None,
                    tool_calls: 0,
                    latest_tokens: None,
                    reasoning_chars: 0,
                    assistant_chars: 0,
                },
            );
        }
    }

    fn on_agent_done(
        &mut self,
        agent_id: AgentId,
        status: AgentStatus,
        tokens: TokenUsage,
        elapsed_ms: u64,
        retry_count: u32,
    ) {
        // Find and remove the agent from whichever phase holds it.
        let entry = self
            .phases
            .iter_mut()
            .find_map(|(_, p)| p.agents.remove(&agent_id));

        let entry = match entry {
            Some(e) => e,
            None => return,
        };

        let (icon, detail) = match status {
            AgentStatus::Ok => {
                let mut parts = vec![
                    fmt_dur(elapsed_ms),
                    tokens.display_split(),
                    format!("{} {}", entry.tool_calls, calls_noun(entry.tool_calls)),
                ];
                if retry_count > 0 {
                    parts.push(format!("{} retries", retry_count));
                }
                (style("✓").green().bold(), parts.join(" · "))
            }
            AgentStatus::Error => (style("✗").red().bold(), "ERROR".into()),
            AgentStatus::Cancelled => (style("⊘").yellow().bold(), "CANCELLED".into()),
            AgentStatus::TimedOut => (style("⏱").yellow().bold(), "TIMEOUT".into()),
        };

        let desc_part = entry
            .description
            .as_deref()
            .map(|d| format!(" · {}", style(d).dim()))
            .unwrap_or_default();
        let line = format!(
            "│   {} {}{} · {}",
            icon,
            entry.label,
            desc_part,
            style(detail).dim()
        );

        // TTY: clear the spinner line, then print the final result.
        // Non-TTY: just print the line.
        if let Some(pb) = &entry.pb {
            pb.finish_and_clear();
        }
        self.print(&line);
    }

    fn on_agent_progress(&mut self, agent_id: &AgentId, delta: &ProgressDelta) {
        // Locate the entry across all phases. We need `&mut` because both the
        // Tokens and ToolCall branches update cached state and re-render the
        // spinner message.
        let entry = self
            .phases
            .values_mut()
            .find_map(|p| p.agents.get_mut(agent_id));
        let entry = match entry {
            Some(e) => e,
            None => return,
        };

        // Update state first — this must happen even when `pb` is `None`
        // (non-TTY mode) so tool_calls and latest_tokens track correctly for
        // the agent-done line and any post-run readers.
        match delta {
            ProgressDelta::Tokens { usage } => {
                entry.latest_tokens = Some(*usage);
            }
            ProgressDelta::ToolCall { .. } => {
                entry.tool_calls += 1;
            }
            ProgressDelta::Message { text } => {
                if text.starts_with("[reasoning]") {
                    entry.reasoning_chars += text.len();
                } else {
                    entry.assistant_chars += text.len();
                }
            }
            _ => {}
        }

        // Re-render the spinner message only when we have one.
        if let Some(pb) = &entry.pb {
            match delta {
                ProgressDelta::Tokens { .. }
                | ProgressDelta::ToolCall { .. }
                | ProgressDelta::Message { .. } => {
                    pb.set_message(format_live(entry));
                }
                _ => {}
            }
        }
    }

    fn on_phase_done(&mut self, phase_id: PhaseId, ok: usize, failed: usize) {
        let (index, elapsed) = match self.phases.get(&phase_id) {
            Some(p) => (p.index, p.start.elapsed()),
            None => return,
        };

        let failed_str = if failed > 0 {
            format!(", {}", style(format!("{} failed", failed)).red())
        } else {
            String::new()
        };

        self.print(&format!(
            "│  {} {} ok{} ({})",
            style(format!("Phase {} done ·", index)).dim(),
            ok,
            failed_str,
            fmt_dur(elapsed.as_millis() as u64),
        ));
        self.print("");
    }

    fn on_run_done(&mut self, status: RunStatus, total_tokens: TokenUsage) {
        let elapsed = self.run_start.map(|s| s.elapsed()).unwrap_or_default();
        let elapsed_ms = elapsed.as_millis() as u64;
        let status_str = match status {
            RunStatus::Completed => style("Completed").green().bold(),
            RunStatus::Failed => style("Failed").red().bold(),
            RunStatus::Cancelled => style("Cancelled").yellow().bold(),
            RunStatus::Partial => style("Partial").yellow().bold(),
        };

        // TTY: the header bar already shows the live elapsed time via the
        // ⏱ suffix. Clear it so the final `╰─ Run done` line (printed below)
        // is the single source of truth for total time. Non-TTY: no header
        // bar to clear; the elapsed time belongs in the final line.
        if let Some(pb) = self.timer_pb.take() {
            pb.finish_and_clear();
        }

        self.print("");
        self.print(&format!(
            "╰─ Run done · {} · {} · {}",
            status_str,
            total_tokens.display_split(),
            fmt_dur(elapsed_ms),
        ));
    }

    // ── Helpers ──────────────────────────────────────────

    fn summary(&self, msg: &str) {
        self.print(&format!("│  {}", style(msg).cyan()));
    }

    fn print(&self, msg: &str) {
        if self.tty {
            let _ = self.mp.println(msg);
        } else {
            println!("{}", msg);
        }
    }
}

// ── Free helpers (testable) ───────────────────────────────

/// Live spinner message for an agent: `label · X tok · Y calls`.
/// Pulled out as a free function so unit tests can verify the format directly
/// without instantiating a `ProgressBar`.
fn format_live(entry: &AgentEntry) -> String {
    let tok = entry.latest_tokens.unwrap_or_default().display_split();
    let mut s = format!(
        "{} · {} · {} {}",
        entry.label,
        tok,
        entry.tool_calls,
        calls_noun(entry.tool_calls)
    );
    if entry.reasoning_chars > 0 || entry.assistant_chars > 0 {
        s.push_str(&format!(
            " · R:{} A:{}",
            fmt_chars(entry.reasoning_chars),
            fmt_chars(entry.assistant_chars)
        ));
    }
    s
}

fn calls_noun(n: u32) -> &'static str {
    if n == 1 {
        "call"
    } else {
        "calls"
    }
}

fn fmt_chars(n: usize) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

// ── Formatting helpers ────────────────────────────────────

fn fmt_dur(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

/// Format elapsed milliseconds as a natural-language clock string that stays
/// visually stable as the value crosses minute/hour boundaries — no digit-
/// count jumps like `HH:MM:SS` would have (e.g. `00:59` → `01:00:00`).
///
/// * `< 60s`       → `"Ns"`        (e.g. `"5s"`, `"59s"`)
/// * `< 3600s`     → `"Mm SSs"`    (e.g. `"1m 23s"`, `"12m 05s"`)
/// * `>= 3600s`    → `"Hh MMm"`    (e.g. `"1h 02m"`, `"2h 15m"`)
///
/// Used for the live header `⏱ ...` suffix and by the timer bar's final
/// message. Truncates sub-second precision (intentional — the value updates
/// once per second).
fn fmt_clock(ms: u64) -> String {
    let total_secs = ms / 1000;
    if total_secs < 60 {
        format!("{}s", total_secs)
    } else if total_secs < 3600 {
        format!("{}m {:02}s", total_secs / 60, total_secs % 60)
    } else {
        format!("{}h {:02}m", total_secs / 3600, (total_secs % 3600) / 60)
    }
}

fn short_id(id: &AgentId) -> String {
    let s = id.to_string();
    s.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro::core::contract::event::{AgentEvent, ProgressDelta, RunStatus};
    use maestro::core::contract::finding::Finding;
    use maestro::core::contract::ids::{AgentId, RunId, TokenUsage};
    use std::sync::{Arc, Mutex};

    fn fake_entry(label: &str, tool_calls: u32, tokens: Option<TokenUsage>) -> AgentEntry {
        AgentEntry {
            label: label.to_string(),
            description: None,
            pb: None,
            tool_calls,
            latest_tokens: tokens,
            reasoning_chars: 0,
            assistant_chars: 0,
        }
    }

    /// Test-only snapshot of an agent's live state.
    #[derive(Debug, Clone, PartialEq)]
    struct AgentSnapshot {
        label: String,
        tool_calls: u32,
        latest_tokens: Option<TokenUsage>,
    }

    /// Test-only accessor: read the current live state for an agent. Returns
    /// `None` after the agent's `AgentDone` event (the entry is removed from
    /// the phase map).
    fn snapshot(r: &PhaseRenderer, agent_id: AgentId) -> Option<AgentSnapshot> {
        r.phases
            .values()
            .find_map(|p| p.agents.get(&agent_id))
            .map(|e| AgentSnapshot {
                label: e.label.clone(),
                tool_calls: e.tool_calls,
                latest_tokens: e.latest_tokens,
            })
    }

    /// Construct a `PhaseRenderer` that captures its stdout output instead of
    /// writing to the real terminal — lets tests assert on what the renderer
    /// would have shown the user.
    fn renderer_with_capture() -> (PhaseRenderer, Arc<Mutex<Vec<u8>>>) {
        // We can't replace stdout from inside println!, so we drive the
        // non-tty path which writes via `println!`. For test capture, we
        // simply trust the entry state and skip line-level checks.
        (PhaseRenderer::new(false), Arc::new(Mutex::new(Vec::new())))
    }

    fn evt_run_started() -> AgentEvent {
        AgentEvent::RunStarted {
            run_id: RunId::now_v7(),
            task: "test".into(),
            ts: chrono::Utc::now(),
        }
    }

    fn evt_phase_started(phase_id: PhaseId) -> AgentEvent {
        AgentEvent::PhaseStarted {
            run_id: RunId::now_v7(),
            phase_id,
            label: "phase".into(),
            planned: 1,
            parent_span_id: None,
            description: None,
            role: None,
        }
    }

    fn evt_agent_started(phase_id: PhaseId, agent_id: AgentId) -> AgentEvent {
        AgentEvent::AgentStarted {
            run_id: RunId::now_v7(),
            phase_id,
            agent_id,
            prompt_preview: "".into(),
            model: None,
            description: None,
            role: None,
            name: Some("researcher-a".into()),
            agent_seq: 0,
        }
    }

    fn evt_tool_call(agent_id: AgentId) -> AgentEvent {
        AgentEvent::AgentProgress {
            run_id: RunId::now_v7(),
            agent_id,
            delta: ProgressDelta::ToolCall {
                name: "bash".into(),
                summary: "ls".into(),
            },
        }
    }

    fn evt_tokens(agent_id: AgentId, total: u64) -> AgentEvent {
        AgentEvent::AgentProgress {
            run_id: RunId::now_v7(),
            agent_id,
            delta: ProgressDelta::Tokens {
                usage: TokenUsage {
                    input: total,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                },
            },
        }
    }

    fn evt_message(agent_id: AgentId, text: &str) -> AgentEvent {
        AgentEvent::AgentProgress {
            run_id: RunId::now_v7(),
            agent_id,
            delta: ProgressDelta::Message { text: text.into() },
        }
    }

    fn evt_agent_done(agent_id: AgentId, tokens: TokenUsage) -> AgentEvent {
        AgentEvent::AgentDone {
            run_id: RunId::now_v7(),
            agent_id,
            status: maestro::core::contract::backend::AgentStatus::Ok,
            tokens,
            elapsed_ms: 1_200,
            name: Some("researcher-a".into()),
            agent_seq: 0,
            output: serde_json::Value::Null,
            findings: Vec::<Finding>::new(),
            prompt: String::new(),
            retry_count: 0,
        }
    }

    fn evt_run_done(status: RunStatus, total: u64) -> AgentEvent {
        AgentEvent::RunDone {
            run_id: RunId::now_v7(),
            status,
            total_tokens: TokenUsage {
                input: total,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
            report: serde_json::json!(null),
        }
    }

    #[test]
    fn handle_tool_call_increments_counter() {
        let (mut r, _cap) = renderer_with_capture();
        let phase_id: PhaseId = 1;
        let agent_id = AgentId::now_v7();

        r.handle(&evt_run_started());
        r.handle(&evt_phase_started(phase_id));
        r.handle(&evt_agent_started(phase_id, agent_id));

        assert_eq!(snapshot(&r, agent_id).unwrap().tool_calls, 0);

        r.handle(&evt_tool_call(agent_id));
        assert_eq!(snapshot(&r, agent_id).unwrap().tool_calls, 1);

        r.handle(&evt_tool_call(agent_id));
        r.handle(&evt_tool_call(agent_id));
        assert_eq!(snapshot(&r, agent_id).unwrap().tool_calls, 3);
    }

    #[test]
    fn handle_tokens_updates_latest_tokens() {
        let (mut r, _cap) = renderer_with_capture();
        let phase_id: PhaseId = 1;
        let agent_id = AgentId::now_v7();

        r.handle(&evt_run_started());
        r.handle(&evt_phase_started(phase_id));
        r.handle(&evt_agent_started(phase_id, agent_id));
        assert_eq!(snapshot(&r, agent_id).unwrap().latest_tokens, None);

        r.handle(&evt_tokens(agent_id, 4_700));
        let s = snapshot(&r, agent_id).unwrap();
        assert_eq!(s.latest_tokens.unwrap().input, 4_700);

        // Second delta overwrites — we keep only the latest.
        r.handle(&evt_tokens(agent_id, 9_500));
        let s = snapshot(&r, agent_id).unwrap();
        assert_eq!(s.latest_tokens.unwrap().input, 9_500);
    }

    #[test]
    fn handle_mixed_progress_independent_state() {
        let (mut r, _cap) = renderer_with_capture();
        let phase_id: PhaseId = 1;
        let agent_id = AgentId::now_v7();

        r.handle(&evt_run_started());
        r.handle(&evt_phase_started(phase_id));
        r.handle(&evt_agent_started(phase_id, agent_id));

        // Interleave: tokens, tool, message, tokens, tool
        r.handle(&evt_tokens(agent_id, 100));
        r.handle(&evt_tool_call(agent_id));
        r.handle(&evt_message(agent_id, "thinking..."));
        r.handle(&evt_tokens(agent_id, 200));
        r.handle(&evt_tool_call(agent_id));

        let s = snapshot(&r, agent_id).unwrap();
        assert_eq!(s.tool_calls, 2);
        assert_eq!(s.latest_tokens.unwrap().input, 200);
        // Message deltas don't touch counters/tokens — verified by reaching
        // this assertion without panics.
    }

    #[test]
    fn handle_agent_done_removes_entry_and_does_not_panic() {
        let (mut r, _cap) = renderer_with_capture();
        let phase_id: PhaseId = 1;
        let agent_id = AgentId::now_v7();

        r.handle(&evt_run_started());
        r.handle(&evt_phase_started(phase_id));
        r.handle(&evt_agent_started(phase_id, agent_id));
        r.handle(&evt_tool_call(agent_id));
        r.handle(&evt_tool_call(agent_id));
        assert_eq!(snapshot(&r, agent_id).unwrap().tool_calls, 2);

        // done event: entry is removed
        r.handle(&evt_agent_done(agent_id, TokenUsage::default()));
        assert_eq!(snapshot(&r, agent_id), None);

        // Progress events after done are no-ops (don't panic)
        r.handle(&evt_tool_call(agent_id));
        r.handle(&evt_tokens(agent_id, 999));
        assert_eq!(snapshot(&r, agent_id), None);
    }

    #[test]
    fn handle_progress_for_unknown_agent_is_noop() {
        let (mut r, _cap) = renderer_with_capture();
        let stranger = AgentId::now_v7();
        // No agent_started for `stranger` — these should not panic.
        r.handle(&evt_tool_call(stranger));
        r.handle(&evt_tokens(stranger, 500));
        r.handle(&evt_message(stranger, "hi"));
        assert_eq!(snapshot(&r, stranger), None);
    }

    #[test]
    fn handle_full_run_smoke() {
        // End-to-end smoke: run → 2 phases → each with 1 agent → progress → done.
        let (mut r, _cap) = renderer_with_capture();
        r.handle(&evt_run_started());

        let phase1: PhaseId = 1;
        let agent_a = AgentId::now_v7();
        r.handle(&evt_phase_started(phase1));
        r.handle(&evt_agent_started(phase1, agent_a));
        r.handle(&evt_tokens(agent_a, 1_000));
        r.handle(&evt_tool_call(agent_a));
        r.handle(&evt_tool_call(agent_a));
        r.handle(&evt_agent_done(
            agent_a,
            TokenUsage {
                input: 1_000,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
        ));

        let phase2: PhaseId = 2;
        let agent_b = AgentId::now_v7();
        r.handle(&evt_phase_started(phase2));
        r.handle(&evt_agent_started(phase2, agent_b));
        r.handle(&evt_tokens(agent_b, 2_500));
        r.handle(&evt_agent_done(
            agent_b,
            TokenUsage {
                input: 2_500,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
        ));

        r.handle(&evt_run_done(RunStatus::Completed, 3_500));

        // Both agents gone, no panic — full pipeline ran.
        assert_eq!(snapshot(&r, agent_a), None);
        assert_eq!(snapshot(&r, agent_b), None);
    }

    #[test]
    fn format_live_zero_calls_no_tokens() {
        let e = fake_entry("researcher-a", 0, None);
        assert_eq!(format_live(&e), "researcher-a · ↑0 ↓0 · 0 calls");
    }

    #[test]
    fn format_live_singular_call() {
        let e = fake_entry("researcher-a", 1, None);
        assert_eq!(format_live(&e), "researcher-a · ↑0 ↓0 · 1 call");
    }

    #[test]
    fn format_live_plural_calls_with_tokens() {
        let e = fake_entry(
            "researcher-a",
            5,
            Some(TokenUsage {
                input: 4_700,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            }),
        );
        assert_eq!(format_live(&e), "researcher-a · ↑4.7k ↓0 · 5 calls");
    }

    #[test]
    fn format_live_uses_b_suffix_for_huge_token_count() {
        let e = fake_entry(
            "x",
            2,
            Some(TokenUsage {
                input: 1_500_000_000,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            }),
        );
        assert_eq!(format_live(&e), "x · ↑1.5B ↓0 · 2 calls");
    }

    #[test]
    fn calls_noun_grammar() {
        assert_eq!(calls_noun(0), "calls");
        assert_eq!(calls_noun(1), "call");
        assert_eq!(calls_noun(2), "calls");
        assert_eq!(calls_noun(99), "calls");
    }

    #[test]
    fn message_delta_updates_spinner_with_char_counts() {
        use indicatif::ProgressDrawTarget;

        let mut r = PhaseRenderer::new(true);
        r.mp = MultiProgress::with_draw_target(ProgressDrawTarget::hidden());

        let phase_id: PhaseId = 1;
        let agent_id = AgentId::now_v7();
        r.handle(&evt_run_started());
        r.handle(&evt_phase_started(phase_id));
        r.handle(&evt_agent_started(phase_id, agent_id));
        r.handle(&evt_tokens(agent_id, 1_200));
        r.handle(&evt_tool_call(agent_id));
        r.handle(&evt_tool_call(agent_id));

        let before_msg = r
            .phases
            .values()
            .find_map(|p| p.agents.get(&agent_id))
            .and_then(|e| e.pb.as_ref().map(|pb| pb.message()))
            .expect("spinner should exist in tty mode");
        assert!(
            before_msg.contains("↑1.2k") && before_msg.contains("2 calls"),
            "metrics msg should show metrics, got: {before_msg:?}",
        );
        assert!(!before_msg.contains("R:"), "no R: before messages arrive");

        r.handle(&evt_message(agent_id, "[reasoning] thinking hard"));
        r.handle(&evt_message(agent_id, "hello world"));

        let after_msg = r
            .phases
            .values()
            .find_map(|p| p.agents.get(&agent_id))
            .and_then(|e| e.pb.as_ref().map(|pb| pb.message()))
            .expect("spinner should still exist");
        assert!(
            after_msg.contains("R:") && after_msg.contains("A:"),
            "spinner should show R:/A: after messages, got: {after_msg:?}",
        );
        assert!(
            after_msg.contains("↑1.2k") && after_msg.contains("2 calls"),
            "metrics must remain stable, got: {after_msg:?}",
        );
    }

    // ── fmt_clock ─────────────────────────────────────────

    #[test]
    fn fmt_clock_sub_minute() {
        assert_eq!(fmt_clock(0), "0s");
        assert_eq!(fmt_clock(500), "0s"); // 500ms truncates
        assert_eq!(fmt_clock(999), "0s");
        assert_eq!(fmt_clock(1_000), "1s");
        assert_eq!(fmt_clock(59_999), "59s");
    }

    #[test]
    fn fmt_clock_sub_hour() {
        assert_eq!(fmt_clock(60_000), "1m 00s");
        assert_eq!(fmt_clock(83_000), "1m 23s");
        // Sub-second precision within a minute truncates (intentional —
        // the live timer only updates once per second).
        assert_eq!(fmt_clock(83_500), "1m 23s");
        assert_eq!(fmt_clock(12 * 60_000 + 5_000), "12m 05s");
        assert_eq!(fmt_clock(59 * 60_000 + 59_000), "59m 59s");
    }

    #[test]
    fn fmt_clock_sub_day() {
        assert_eq!(fmt_clock(60 * 60_000), "1h 00m");
        assert_eq!(fmt_clock(60 * 60_000 + 23 * 60_000), "1h 23m");
        assert_eq!(fmt_clock(2 * 3_600_000 + 15 * 60_000), "2h 15m");
    }

    // ── tick_elapsed ──────────────────────────────────────

    #[test]
    fn tick_elapsed_before_run_started_is_noop() {
        // No RunStarted was sent — timer_pb is None, tick_elapsed must not
        // touch anything (in particular, must not panic).
        let r = PhaseRenderer::new(true);
        r.tick_elapsed();
    }

    #[test]
    fn tick_elapsed_after_run_done_clears_timer() {
        // Regression: on_run_done must take() the timer_pb so a late
        // tick_elapsed (e.g. a tokio interval fire that races with RunDone)
        // becomes a no-op rather than writing to a finished bar.
        use indicatif::ProgressDrawTarget;

        let mut r = PhaseRenderer::new(true);
        r.mp = MultiProgress::with_draw_target(ProgressDrawTarget::hidden());

        r.handle(&evt_run_started());
        // Sanity: timer_pb should be present in TTY mode after RunStarted.
        assert!(
            r.timer_pb.is_some(),
            "timer_pb should be Some in TTY mode after RunStarted",
        );

        r.handle(&evt_run_done(RunStatus::Completed, 0));
        assert!(
            r.timer_pb.is_none(),
            "timer_pb should be cleared by on_run_done",
        );

        // Calling tick_elapsed after on_run_done must not panic.
        r.tick_elapsed();
    }
}
