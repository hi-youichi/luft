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
//! │   ✓ researcher-a  1.2s  842 tok
//! │   ✓ researcher-b  0.8s  510 tok
//! │   ✗ researcher-c  3.1s  ERROR
//! │  Phase 1 done · 2 ok, 1 failed (3.1s)
//!
//! ├── Phase 2 · Synthesis
//! │  converge · 2 rounds → converged ✓ (2 surviving, 4.2s)
//! │  Phase 2 done · 2 ok (4.2s)
//!
//! ╰─ Run done · Completed · 4652 tok · 12.6s
//! ```

use std::collections::HashMap;
use std::time::{Duration, Instant};

use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use maestro::core::contract::backend::AgentStatus;
use maestro::core::contract::event::{AgentEvent, ProgressDelta, RunStatus};
use maestro::core::contract::ids::{AgentId, PhaseId, TokenUsage};

// ── Data model ────────────────────────────────────────────

struct AgentEntry {
    label: String,
    description: Option<String>,
    pb: Option<ProgressBar>,
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
    phases: HashMap<PhaseId, PhaseEntry>,
    phase_order: Vec<PhaseId>,
}

impl PhaseRenderer {
    pub fn new(tty: bool) -> Self {
        Self {
            tty,
            mp: MultiProgress::new(),
            run_start: None,
            phases: HashMap::new(),
            phase_order: Vec::new(),
        }
    }

    /// Dispatch an event to the appropriate handler.
    pub fn handle(&mut self, evt: &AgentEvent) {
        match evt {
            AgentEvent::RunStarted { task, .. } => self.on_run_started(task),
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
                ..
            } => {
                self.on_agent_done(*agent_id, status.clone(), *tokens, *elapsed_ms);
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

    fn on_run_started(&mut self, task: &str) {
        self.run_start = Some(Instant::now());
        self.print(&format!("╭─ Run: {}", style(task).bold()));
        self.print("");
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

    fn on_phase_started(&mut self, phase_id: PhaseId, label: &str, planned: usize, description: Option<&str>) {
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
                },
            );
        } else {
            phase.agents.insert(
                agent_id,
                AgentEntry {
                    label,
                    description: display_desc,
                    pb: None,
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
            AgentStatus::Ok => (
                style("✓").green().bold(),
                format!("{} · {} tok", fmt_dur(elapsed_ms), tokens.total()),
            ),
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

    fn on_agent_progress(&self, agent_id: &AgentId, delta: &ProgressDelta) {
        let entry = self
            .phases
            .values()
            .find_map(|p| p.agents.get(agent_id));
        let (pb, label) = match entry {
            Some(e) => (&e.pb, &e.label),
            None => return,
        };
        let pb = match pb {
            Some(pb) => pb,
            None => return,
        };
        match delta {
            ProgressDelta::Tokens { usage } => {
                let msg = format!("{} · {} tok", label, usage.total());
                pb.set_message(msg);
            }
            ProgressDelta::Message { text } => {
                let preview = if text.len() > 40 {
                    format!("{}…", &text[..37])
                } else {
                    text.clone()
                };
                pb.set_message(format!("{} · {}", label, style(preview).dim()));
            }
            _ => {}
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
        let status_str = match status {
            RunStatus::Completed => style("Completed").green().bold(),
            RunStatus::Failed => style("Failed").red().bold(),
            RunStatus::Cancelled => style("Cancelled").yellow().bold(),
            RunStatus::Partial => style("Partial").yellow().bold(),
        };
        self.print("");
        self.print(&format!(
            "╰─ Run done · {} · {} tok · {}",
            status_str,
            total_tokens.total(),
            fmt_dur(elapsed.as_millis() as u64),
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

// ── Formatting helpers ────────────────────────────────────

fn fmt_dur(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

fn short_id(id: &AgentId) -> String {
    let s = id.to_string();
    s.chars().take(8).collect()
}
