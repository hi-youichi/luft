```lua
-- workflow: run-tui.lua
-- Implements the real-time ratatui TUI for `maestro run` as specified in
-- docs/design/run-tui.md.  Orchestrates subagents to:
--   1. Analyze the existing codebase
--   2. Add ratatui + crossterm deps
--   3. Add --tui / --no-tui CLI flags
--   4. Wire TTY detection + dispatch into commands/run.rs
--   5. Create commands/run_tui.rs (the TUI app)
--   6. Fix test compilation
--   7. Run cargo check

budget(300000, 50)

-- ── Schemas ───────────────────────────────────────────────────────────────

local ANALYSIS_SCHEMA = {
  type = "object",
  properties = {
    summary = { type = "string" },
    imports_needed = { type = "array", items = { type = "string" } },
    main_rs_lines = { type = "object",
      properties = {
        run_args_start = { type = "integer" },
        run_args_end = { type = "integer" },
        dispatch_start = { type = "integer" },
        dispatch_end = { type = "integer" },
      },
      required = { "run_args_start", "run_args_end", "dispatch_start", "dispatch_end" }
    },
    run_rs_functions = { type = "array", items = { type = "object",
      properties = {
        name = { type = "string" },
        start_line = { type = "integer" },
        end_line = { type = "integer" },
        signature = { type = "string" }
      },
      required = { "name", "start_line", "end_line" }
    } },
    -- The test usages of RunArgs in main.rs tests
    run_args_test_locations = { type = "array", items = { type = "object",
      properties = {
        test_name = { type = "string" },
        line = { type = "integer" },
      },
      required = { "test_name", "line" }
    } }
  },
  required = { "summary", "run_rs_functions", "main_rs_lines" }
}

local EDIT_RESULT = {
  type = "object",
  properties = {
    file = { type = "string" },
    changed = { type = "boolean" },
    summary = { type = "string" }
  },
  required = { "file", "changed", "summary" }
}

local NEW_FILE_RESULT = {
  type = "object",
  properties = {
    file = { type = "string" },
    lines = { type = "integer" },
    summary = { type = "string" }
  },
  required = { "file", "lines", "summary" }
}

local CHECK_RESULT = {
  type = "object",
  properties = {
    success = { type = "boolean" },
    output = { type = "string" },
    errors = { type = "array", items = { type = "string" } }
  },
  required = { "success", "output" }
}

-- ── Phase 1: Analysis ────────────────────────────────────────────────────

local p1 = phase("analyze codebase", 1)

local analysis = agent({
  prompt = [[Read the following files in the maestro project at /Users/apple/dev/maestro and return a detailed analysis:

1. src/main.rs (especially RunArgs struct around lines 88-130, Commands enum, dispatch function)
2. src/commands/run.rs (full file, all function signatures and test locations)
3. src/commands/mod.rs (module declarations)
4. src/core/contract/event.rs (AgentEvent enum, ProgressDelta)
5. src/core/state.rs (RunCheckpoint struct)
6. src/service/phases.rs (PhasesView, PhaseRow, AgentRow, build_phases_view)
7. src/service/run.rs (PreparedRun struct, the execute function signature)
8. Cargo.toml (current dependencies)
9. src/lib.rs (module exports)

Return a structured summary with:
- summary: overall analysis
- imports_needed: list of crates not yet in Cargo.toml that we need
- main_rs_lines: exact line numbers for RunArgs struct, Commands enum, and dispatch
- run_rs_functions: each public/async function in run.rs with name, start_line, end_line, signature
- run_args_test_locations: each place in main.rs tests where RunArgs is constructed (test name + line number)]],
  schema = ANALYSIS_SCHEMA
})

if not analysis.ok then
  report({ error = "analysis failed: " .. (analysis.status or "unknown"), phase = "analysis" })
  return
end

log("analysis complete: " .. analysis.output.summary)

-- ── Phase 2: Implement Cargo.toml ────────────────────────────────────────

local p2 = phase("add ratatui + crossterm deps", 1)

local cargo_edit = agent({
  prompt = [[Edit /Users/apple/dev/maestro/Cargo.toml to add ratatui 0.29 and crossterm 0.28 as dependencies.

Insert them AFTER the `tempfile = "3"` line on line 44 and BEFORE the blank-line comment "# Storage (SQLite via sqlx)" on line 46.

Add these two lines:
ratatui = { version = "0.29", default-features = false, features = ["crossterm"] }
crossterm = "0.28"

Also add `crossterm` to the tokio features (not needed, tokio already has "full").

Return a summary of what changed.]],
  schema = EDIT_RESULT
})

if not cargo_edit.ok then
  report({ error = "Cargo.toml edit failed: " .. (cargo_edit.status or "unknown") })
  return
end

log("cargo.toml: " .. cargo_edit.output.summary)

-- ── Phase 3: main.rs CLI flags ───────────────────────────────────────────

local p3 = phase("add --tui / --no-tui flags", 1)

local main_edit = agent({
  prompt = [[Edit the RunArgs struct in /Users/apple/dev/maestro/src/main.rs to add two new CLI flags.

The RunArgs struct is approximately at lines 88-130. Add the following two fields INSIDE the struct, after the `extra_args` field (before the closing `}`):

    /// Force TUI mode (fails if not a TTY).
    #[arg(long, help = "Force TUI mode (requires a TTY; errors otherwise)")]
    tui: bool,

    /// Force no-TUI mode (plain log output even on TTY).
    #[arg(long, help = "Disable TUI (plain log output even on TTY)")]
    no_tui: bool,

Also add a validation note: --tui and --no-tui are mutually exclusive but clap does not enforce this at parse time; we handle it in the handler.

Return a summary.]],
  schema = EDIT_RESULT
})

if not main_edit.ok then
  report({ error = "main.rs edit failed: " .. (main_edit.status or "unknown") })
  return
end

log("main.rs flags: " .. main_edit.output.summary)

-- ── Phase 4: commands/run.rs dispatch ────────────────────────────────────

local p4 = phase("wire TTY detection + dispatch", 1)

local run_edit = agent({
  prompt = [[Edit /Users/apple/dev/maestro/src/commands/run.rs to wire in TUI mode.

The file currently has:

1. At the top: `use crate::RunArgs;` — fine, keep it.
2. `run_workflow()` function (lines 17-106) that prepares and then calls `run_headless(...)` on line 105.
3. `run_headless()` function (lines 163-227).

**Changes needed:**

### A. Add import at top of file (after existing imports):
```rust
use crate::commands::run_tui;
use std::io::IsTerminal;
```

### B. In `run_workflow()`, BEFORE the line `run_headless(...)` on line 105, add TTY detection logic:

```rust
    // ── TUI / TTY detection ──────────────────────────────────────
    let use_tui = match (args.tui, args.no_tui) {
        (true, true) => anyhow::bail!("--tui and --no-tui are mutually exclusive"),
        (true, false) => {
            if !std::io::stdout().is_terminal() {
                anyhow::bail!("--tui requires a TTY");
            }
            true
        }
        (false, true) => false,
        (false, false) => std::io::stdout().is_terminal(),
    };

    if use_tui {
        return run_tui::run_tui(prepared, run_ctx, spec.script, args.output, logger).await;
    }
```

Replace the line:
```rust
    run_headless(run_ctx, prepared.runtime, spec.script, args.output, logger).await
```
with:
```rust
    // Headless mode (no TTY or --no-tui)
    run_headless(run_ctx, prepared.runtime, spec.script, args.output, logger).await
```

### C. Update `run_headless` to be `pub(super)` instead of private:

Change line 163:
```rust
async fn run_headless(
```
to:
```rust
pub(super) async fn run_headless(
```

Return a summary of the exact changes made, including line numbers.]],
  schema = EDIT_RESULT
})

if not run_edit.ok then
  report({ error = "run.rs edit failed: " .. (run_edit.status or "unknown") })
  return
end

log("run.rs dispatch: " .. run_edit.output.summary)

-- ── Phase 5: Create commands/run_tui.rs ──────────────────────────────────

local p5 = phase("create run_tui.rs", 1)

local tui_schema = {
  type = "object",
  properties = {
    file = { type = "string" },
    lines = { type = "integer" },
    summary = { type = "string" },
    has_render = { type = "boolean" },
    has_event_loop = { type = "boolean" },
    has_tests = { type = "boolean" }
  },
  required = { "file", "lines", "summary", "has_render", "has_event_loop", "has_tests" }
}

local tui_file = agent({
  prompt = [[Create a NEW file at /Users/apple/dev/maestro/src/commands/run_tui.rs

This is the real-time ratatui TUI for `maestro run`. Read these reference files first:
- /Users/apple/dev/maestro/docs/design/run-tui.md (the full design doc)
- /Users/apple/dev/maestro/src/core/contract/event.rs (AgentEvent enum)
- /Users/apple/dev/maestro/src/core/state.rs (RunCheckpoint, CheckpointStatus, AgentResultCache)
- /Users/apple/dev/maestro/src/service/phases.rs (PhasesView, PhaseRow, AgentRow, RunHeader, PhaseStatus, build_phases_view)
- /Users/apple/dev/maestro/src/commands/run.rs (run_headless function, write_report)
- /Users/apple/dev/maestro/src/core/contract/backend.rs (RunContext)
- /Users/apple/dev/maestro/src/commands/event_log.rs (EventLogger)

Write the COMPLETE file with all of the following:

```rust
//! `run` TUI mode — full-screen ratatui real-time progress view.
//!
//! Architecture:
//!   - tokio worker thread runs `runtime.execute(script)`
//!   - main thread runs ratatui event loop
//!   - communication via broadcast::Receiver<AgentEvent>

use crate::commands::event_log::EventLogger;
use crate::commands::run::run_headless;
use crate::core::contract::backend::RunContext;
use crate::core::contract::event::{AgentEvent, ProgressDelta};
use crate::core::contract::ids::{AgentId, PhaseId, RunId};
use crate::core::state::RunCheckpoint;
use crate::runtime::Runtime;
use crate::service::phases::{build_phases_view, AgentRow, PhaseRow, PhaseStatus, PhasesView, RunHeader};
use crate::service::run as svc;
use anyhow::Result;
use ratatui::{
    Frame,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    symbols,
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Gauge, List, ListItem, Paragraph, Wrap},
    Terminal,
};
use std::collections::HashMap;
use std::io::{self, stdout, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::broadcast;

// ── Running agent state (live progress, not in checkpoint) ────────

#[derive(Debug, Clone)]
struct RunningAgentState {
    agent_id: AgentId,
    phase_id: PhaseId,
    tokens: u64,
    tool_count: usize,
    last_message: String,
}

// ── TuiApp ────────────────────────────────────────────────────────

struct TuiApp {
    checkpoint: RunCheckpoint,
    events: Vec<AgentEvent>,
    view: PhasesView,
    finished: bool,
    scroll: usize,
    show_agent_detail: bool,
    running_agents: HashMap<AgentId, RunningAgentState>,
    warnings: Vec<String>,
    freeze_until: Option<std::time::Instant>,
    /// Set to true when user hits q — shows confirm dialog
    abort_requested: bool,
}

impl TuiApp {
    fn new() -> Self {
        Self {
            checkpoint: RunCheckpoint::default(),
            events: Vec::new(),
            view: PhasesView {
                run: RunHeader {
                    run_id: RunId::default(),
                    task: String::new(),
                    status: crate::core::state::CheckpointStatus::Running,
                    current_phase: 0,
                    total_phases: 0,
                    total_tokens: 0,
                    elapsed_secs: None,
                    created_at: 0,
                },
                source: crate::service::phases::PhasesSource::EventsFallback,
                phases: Vec::new(),
            },
            finished: false,
            scroll: 0,
            show_agent_detail: false,
            running_agents: HashMap::new(),
            warnings: Vec::new(),
            freeze_until: None,
            abort_requested: false,
        }
    }

    fn handle_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::AgentStarted { agent_id, phase_id, .. } => {
                self.running_agents.insert(*agent_id, RunningAgentState {
                    agent_id: *agent_id,
                    phase_id: *phase_id,
                    tokens: 0,
                    tool_count: 0,
                    last_message: String::new(),
                });
            }
            AgentEvent::AgentProgress { agent_id, delta } => {
                if let Some(state) = self.running_agents.get_mut(agent_id) {
                    match delta {
                        ProgressDelta::Tokens { usage } => {
                            state.tokens = usage.total();
                        }
                        ProgressDelta::ToolCall { .. } => {
                            state.tool_count += 1;
                        }
                        ProgressDelta::Message { text } => {
                            state.last_message = truncate(text, 60);
                        }
                        ProgressDelta::FileEdit { .. } => {}
                    }
                }
            }
            AgentEvent::AgentDone { agent_id, .. } => {
                self.running_agents.remove(agent_id);
            }
            _ => {}
        }

        // Always update checkpoint from event, then rebuild view.
        // We do NOT call self.checkpoint.update_from_event — that is done
        // by the journal forwarder. Instead we let build_phases_view
        // derive state from the event list.
        self.events.push(event.clone());
        self.rebuild_view();

        // Check for terminal events.
        if matches!(event, AgentEvent::RunDone { .. }) {
            self.finished = true;
            self.freeze_until = Some(std::time::Instant::now() + Duration::from_secs(3));
        }
    }

    fn rebuild_view(&mut self) {
        self.view = build_phases_view(&self.checkpoint, &self.events);
        // Patch in live running-agent state into the view.
        for phase in &mut self.view.phases {
            for agent in &mut phase.agents {
                if agent.status == "running" {
                    // Try to match by short_id prefix (8 chars of UUID)
                    for (_aid, state) in &self.running_agents {
                        let short = format!("{:.8}", state.agent_id);
                        if agent.short_id == short {
                            agent.tokens = Some(state.tokens);
                            agent.tool_count = Some(state.tool_count);
                            agent.last_message = Some(state.last_message.clone());
                        }
                    }
                }
            }
            // Also inject running agents that are NOT yet in the view
            // (they started but build_phases_view hasn't seen them).
            for (_aid, state) in &self.running_agents {
                if state.phase_id == phase.phase_id {
                    let short = format!("{:.8}", state.agent_id);
                    let already = phase.agents.iter().any(|a| a.short_id == short);
                    if !already {
                        phase.agents.push(AgentRow {
                            short_id: short,
                            status: "running".to_string(),
                            tokens: Some(state.tokens),
                            findings: 0,
                            tool_count: Some(state.tool_count),
                            last_message: Some(state.last_message.clone()),
                        });
                    }
                }
            }
        }
    }

    fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }
}

// ── Rendering ─────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

fn status_icon(status: &PhaseStatus) -> &'static str {
    match status {
        PhaseStatus::Pending => "○",
        PhaseStatus::Running => "◐",
        PhaseStatus::Completed => "●",
        PhaseStatus::Failed => "✕",
    }
}

fn status_color(status: &PhaseStatus) -> Color {
    match status {
        PhaseStatus::Pending => Color::Gray,
        PhaseStatus::Running => Color::Blue,
        PhaseStatus::Completed => Color::Green,
        PhaseStatus::Failed => Color::Red,
    }
}

fn agent_status_icon(status: &str) -> &'static str {
    match status {
        "running" => "◐",
        "completed" => "●",
        "failed" => "✕",
        "cancelled" => "○",
        _ => "?",
    }
}

fn render(app: &TuiApp, frame: &mut Frame) {
    let area = frame.area();
    let constraints = if area.height >= 6 {
        vec![
            Constraint::Length(3),   // Header
            Constraint::Min(1),      // Phases list
            Constraint::Length(1),   // Status bar
        ]
    } else {
        vec![Constraint::Min(1)]
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    // ── Header ───────────────────────────────────────────────────
    render_header(app, frame, chunks[0]);

    // ── Phases list ──────────────────────────────────────────────
    render_phases(app, frame, chunks[1]);

    // ── Status bar ───────────────────────────────────────────────
    render_status_bar(app, frame, chunks[2]);
}

fn render_header(app: &TuiApp, frame: &mut Frame, area: Rect) {
    let run = &app.view.run;
    let elapsed = run.elapsed_secs.map(|s| format!("{:.1}s", s)).unwrap_or_else(|| "--".to_string());
    let phase_info = if run.total_phases > 0 {
        format!("phase {}/{}", run.current_phase, run.total_phases)
    } else {
        String::new()
    };
    let status_str = match run.status {
        crate::core::state::CheckpointStatus::Running => "Running",
        crate::core::state::CheckpointStatus::Completed => "Completed",
        crate::core::state::CheckpointStatus::Failed => "Failed",
        crate::core::state::CheckpointStatus::Cancelled => "Cancelled",
    };

    let header_text = format!(
        "Run {:.8}  status={}  {}  {} tok  {}",
        run.run_id,
        status_str,
        phase_info,
        run.total_tokens,
        elapsed,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" maestro run — {} ", run.task))
        .border_type(BorderType::Rounded);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from(Span::raw(header_text)),
        Line::from(Span::raw(format!("Task: {}", run.task))),
    ];
    let paragraph = Paragraph::new(Text::from(lines)).style(Style::default());
    frame.render_widget(paragraph, inner);
}

fn render_phases(app: &TuiApp, frame: &mut Frame, area: Rect) {
    let phases = &app.view.phases;
    if phases.is_empty() {
        let paragraph = Paragraph::new(Text::from(Line::from(Span::raw(
            " Waiting for phases..."
        ))));
        frame.render_widget(paragraph, area);
        return;
    }

    let mut items: Vec<ListItem> = Vec::new();
    for phase in phases {
        let icon = status_icon(&phase.status);
        let color = status_color(&phase.status);
        let elapsed = phase.elapsed_secs.map(|s| format!("{:.1}s", s)).unwrap_or_default();
        let detail = phase.detail.as_deref().unwrap_or("");

        let phase_line = Line::from(vec![
            Span::styled(format!(" {} ", icon), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::raw(format!("Phase {}/{}  {}  ", phase.phase_id, app.view.run.total_phases, phase.label)),
            Span::styled(
                format!("[{}]", phase.status.as_str()),
                Style::default().fg(color),
            ),
            Span::raw(format!("  {}", elapsed)),
        ]);
        items.push(ListItem::new(phase_line));

        if !detail.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled(
                format!("    ┊ {}", detail),
                Style::default().fg(Color::Gray),
            ))));
        }

        // Agent rows
        for agent in &phase.agents {
            let agent_icon = agent_status_icon(&agent.status);
            let tokens_str = agent.tokens.map(|t| format!("{} tok", t)).unwrap_or_else(|| "-- tok".to_string());
            let tools_str = agent.tool_count.map(|t| format!("tools={}", t)).unwrap_or_else(|| String::new());
            let msg_preview = agent.last_message.as_deref().unwrap_or("");

            let agent_line = if app.show_agent_detail {
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled(agent_icon, Style::default().fg(match agent.status.as_str() {
                        "running" => Color::Blue,
                        "completed" => Color::Green,
                        _ => Color::Red,
                    })),
                    Span::raw(format!(" {}  {}  {}", agent.short_id, tokens_str, tools_str)),
                ])
            } else {
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled(agent_icon, Style::default().fg(match agent.status.as_str() {
                        "running" => Color::Blue,
                        "completed" => Color::Green,
                        _ => Color::Red,
                    })),
                    Span::raw(format!(" {}  {}  ", agent.short_id, tokens_str)),
                    Span::styled(msg_preview, Style::default().fg(Color::Gray)),
                ])
            };
            items.push(ListItem::new(agent_line));
        }
    }

    let list = List::new(items).block(Block::default().borders(Borders::NONE));
    frame.render_widget(list, area);
}

fn render_status_bar(app: &TuiApp, frame: &mut Frame, area: Rect) {
    let text = if let Some(freeze_until) = app.freeze_until {
        let remaining = freeze_until.saturating_duration_since(std::time::Instant::now());
        format!("Run completed. Exiting in {}s...", remaining.as_secs() + 1)
    } else if app.abort_requested {
        "Abort workflow? (y/n)".to_string()
    } else {
        "q: quit  ↑↓: scroll  Tab: toggle agent detail".to_string()
    };

    let block = Block::default()
        .borders(Borders::TOP)
        .border_type(BorderType::Plain);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let paragraph = Paragraph::new(Line::from(Span::raw(text)))
        .style(Style::default().fg(Color::Cyan));
    frame.render_widget(paragraph, inner);
}

// ── Terminal helpers ──────────────────────────────────────────────

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
    )?;
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture,
    )?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

// ── Keyboard polling (non-blocking) ───────────────────────────────

async fn poll_keyboard() -> Result<bool> {
    tokio::task::spawn_blocking(|| {
        use crossterm::event::{Event, KeyCode, KeyModifiers};
        let timeout = Duration::from_millis(100);
        if !crossterm::event::poll(timeout).unwrap_or(false) {
            return Ok(false);
        }
        match crossterm::event::read() {
            Ok(Event::Key(key)) => Ok(match key.code {
                KeyCode::Char('q') | KeyCode::Char('Q') => true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
                _ => true, // any key wakes us
            }),
            Ok(_) => Ok(false),
            Err(_) => Ok(false),
        }
    })
    .await?
}

fn read_key() -> Result<crossterm::event::KeyEvent> {
    use crossterm::event::{Event, KeyEvent};
    loop {
        match crossterm::event::read() {
            Ok(Event::Key(key)) => return Ok(key),
            Ok(_) => continue,
            Err(e) => anyhow::bail!("keyboard read error: {}", e),
        }
    }
}

fn confirm_exit() -> Result<bool> {
    use crossterm::event::{Event, KeyCode};
    loop {
        match crossterm::event::read() {
            Ok(Event::Key(key)) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => return Ok(true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => return Ok(false),
                _ => {}
            },
            Ok(_) => {}
            Err(e) => anyhow::bail!("confirm read error: {}", e),
        }
    }
}

// ── Main TUI entry point ──────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub async fn run_tui(
    prepared: svc::PreparedRun,
    run_ctx: RunContext,
    script: String,
    output: Option<PathBuf>,
    logger: Option<EventLogger>,
) -> Result<()> {
    let run_id = run_ctx.run_id;

    // Initial checkpoint from journal
    let mut app = TuiApp::new();
    app.checkpoint = prepared.journal.store().get_checkpoint().unwrap_or_else(|| RunCheckpoint {
        run_id,
        task: String::new(),
        status: crate::core::state::CheckpointStatus::Running,
        current_phase: 0,
        completed_phases: Vec::new(),
        agent_results: HashMap::new(),
        findings: Vec::new(),
        total_tokens: 0,
        created_at: 0,
        updated_at: 0,
        workflow_meta: None,
    });

    // Subscribe to events
    let mut rx = run_ctx.events.subscribe();

    // Spawn workflow execution
    let run_handle = tokio::spawn(async move {
        svc::execute(&run_ctx, prepared.runtime, script).await
    });

    // Setup terminal
    let mut terminal = match setup_terminal() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("warning: TUI init failed ({}), falling back to headless mode", e);
            run_headless(run_ctx, prepared.runtime, script, output, logger).await?;
            return Ok(());
        }
    };

    // Initial render
    let _ = terminal.draw(|f| render(&app, f));

    // Event loop
    let result: Result<serde_json::Value> = loop {
        // Render
        let _ = terminal.draw(|f| render(&app, f));

        // Check freeze timer
        if let Some(freeze_until) = app.freeze_until {
            if std::time::Instant::now() >= freeze_until {
                break {
                    let exec_result = run_handle.await??;
                    exec_result
                };
            }
        }

        // Wait for either keyboard event or agent event
        tokio::select! {
            _key_hit = poll_keyboard() => {
                if !app.finished || app.freeze_until.is_some() {
                    // Read the actual key
                    let key = read_key()?;
                    use crossterm::event::KeyCode;
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => {
                            if app.finished {
                                // Already done, just exit
                                break {
                                    let exec_result = run_handle.await??;
                                    exec_result
                                };
                            }
                            app.abort_requested = true;
                            let _ = terminal.draw(|f| render(&app, f));
                            if confirm_exit()? {
                                run_ctx.cancel.cancel();
                                break Ok(serde_json::json!({
                                    "status": "cancelled",
                                    "run_id": run_id.to_string(),
                                }));
                            }
                            app.abort_requested = false;
                        }
                        KeyCode::Up => app.scroll_up(),
                        KeyCode::Down => app.scroll_down(),
                        KeyCode::Tab => app.show_agent_detail = !app.show_agent_detail,
                        KeyCode::Char('r') => {
                            // Force redraw (refresh)
                        }
                        _ => {}
                    }
                }
            }
            ev = rx.recv() => {
                match ev {
                    Ok(event) => {
                        app.handle_event(&event);
                        if app.finished {
                            // Keep rendering during freeze period
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break {
                            let exec_result = run_handle.await??;
                            exec_result
                        };
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        app.warnings.push(format!("skipped {} events", n));
                    }
                }
            }
        }
    };

    // Restore terminal
    restore_terminal(&mut terminal)?;

    // Print final report
    match result {
        Ok(report) => {
            if let Some(path) = &output {
                crate::commands::run::write_report(path, &report)?;
                eprintln!("Report written to {}", path.display());
            }
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "type": "report",
                    "run_id": run_id.to_string(),
                    "report": report,
                }))?
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "workflow execution failed");
            eprintln!("Execution error: {}", e);
        }
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::ids::TokenUsage;
    use crate::core::state::CheckpointStatus;

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let long = "a".repeat(100);
        let result = truncate(&long, 10);
        assert_eq!(result.len(), 13); // 10 chars + "..."
        assert!(result.starts_with("aaaaaaaaaa"));
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_status_icon_variants() {
        assert_eq!(status_icon(&PhaseStatus::Pending), "○");
        assert_eq!(status_icon(&PhaseStatus::Running), "◐");
        assert_eq!(status_icon(&PhaseStatus::Completed), "●");
        assert_eq!(status_icon(&PhaseStatus::Failed), "✕");
    }

    #[test]
    fn test_status_color_variants() {
        assert_eq!(status_color(&PhaseStatus::Pending), Color::Gray);
        assert_eq!(status_color(&PhaseStatus::Running), Color::Blue);
        assert_eq!(status_color(&PhaseStatus::Completed), Color::Green);
        assert_eq!(status_color(&PhaseStatus::Failed), Color::Red);
    }

    #[test]
    fn test_agent_status_icon_mapping() {
        assert_eq!(agent_status_icon("running"), "◐");
        assert_eq!(agent_status_icon("completed"), "●");
        assert_eq!(agent_status_icon("failed"), "✕");
        assert_eq!(agent_status_icon("cancelled"), "○");
        assert_eq!(agent_status_icon("unknown"), "?");
    }

    #[test]
    fn test_app_scroll_clamps() {
        let mut app = TuiApp::new();
        assert_eq!(app.scroll, 0);
        app.scroll_up();
        assert_eq!(app.scroll, 0); // clamped at 0
        app.scroll_down();
        assert_eq!(app.scroll, 1);
    }

    #[test]
    fn test_app_agent_progress_tracking() {
        let mut app = TuiApp::new();
        let agent_id = AgentId::now_v7();

        // Simulate AgentStarted
        let started = AgentEvent::AgentStarted {
            run_id: RunId::now_v7(),
            phase_id: 1,
            agent_id,
            prompt_preview: "test".into(),
            model: None,
        };
        app.handle_event(&started);
        assert!(app.running_agents.contains_key(&agent_id));
        assert_eq!(app.running_agents[&agent_id].tokens, 0);

        // Simulate Tokens progress
        let tokens_event = AgentEvent::AgentProgress {
            run_id: RunId::now_v7(),
            agent_id,
            delta: ProgressDelta::Tokens { usage: TokenUsage { input: 100, output: 50, cache_read: 0, cache_write: 0 } },
        };
        app.handle_event(&tokens_event);
        assert_eq!(app.running_agents[&agent_id].tokens, 150);

        // Simulate ToolCall progress
        let tool_event = AgentEvent::AgentProgress {
            run_id: RunId::now_v7(),
            agent_id,
            delta: ProgressDelta::ToolCall { name: "read".into(), summary: "reading file".into() },
        };
        app.handle_event(&tool_event);
        assert_eq!(app.running_agents[&agent_id].tool_count, 1);

        // Simulate Message progress
        let msg_event = AgentEvent::AgentProgress {
            run_id: RunId::now_v7(),
            agent_id,
            delta: ProgressDelta::Message { text: "analyzing data".into() },
        };
        app.handle_event(&msg_event);
        assert_eq!(app.running_agents[&agent_id].last_message, "analyzing data");

        // Simulate AgentDone — should remove from running_agents
        let done = AgentEvent::AgentDone {
            run_id: RunId::now_v7(),
            agent_id,
            status: crate::core::contract::backend::AgentStatus::Ok,
            tokens: TokenUsage { input: 100, output: 50, cache_read: 0, cache_write: 0 },
            elapsed_ms: 1000,
        };
        app.handle_event(&done);
        assert!(!app.running_agents.contains_key(&agent_id));
    }

    #[test]
    fn test_app_finishes_on_run_done() {
        let mut app = TuiApp::new();
        let run_id = RunId::now_v7();
        let done = AgentEvent::RunDone {
            run_id,
            status: crate::core::contract::event::RunStatus::Completed,
            total_tokens: TokenUsage::default(),
            report: serde_json::json!({"ok": true}),
            ts: chrono::Utc::now(),
        };
        app.handle_event(&done);
        assert!(app.finished);
        assert!(app.freeze_until.is_some());
    }
}
```

IMPORTANT: Write the COMPLETE file. The file should be fully functional, not a skeleton. Make sure all imports are correct, all types match the existing codebase exactly, and the code compiles.

After writing the file, return a summary with exact line count and confirmation of which features are implemented.]]],
  schema = tui_schema,
  timeout_ms = 300000
})

if not tui_file.ok then
  report({ error = "run_tui.rs creation failed: " .. (tui_file.status or "unknown") })
  return
end

log("run_tui.rs: " .. tui_file.output.lines .. " lines written")
if not tui_file.output.has_render then
  log("WARNING: render function may be missing", "warn")
end
if not tui_file.output.has_event_loop then
  log("WARNING: event loop may be missing", "warn")
end

-- ── Phase 6: Register module in commands/mod.rs ──────────────────────────

local p6 = phase("register run_tui module", 1)

local mod_edit = agent({
  prompt = [[Edit /Users/apple/dev/maestro/src/commands/mod.rs to add the run_tui module declaration.

Add this line after the `pub mod run;` line (line 10):
pub mod run_tui;

Return the file path and a summary.]],
  schema = EDIT_RESULT
})

if not mod_edit.ok then
  report({ error = "mod.rs edit failed: " .. (mod_edit.status or "unknown") })
  return
end

log("mod.rs: " .. mod_edit.output.summary)

-- ── Phase 7: Fix RunArgs tests in main.rs ────────────────────────────────

local p7 = phase("fix test RunArgs constructions", 1)

local test_fix = agent({
  prompt = [[Edit /Users/apple/dev/maestro/src/main.rs to add the two new `tui` and `no_tui` fields to every RunArgs construction in the test module.

Search for each occurrence of `RunArgs {` in the test module (below `mod tests {`). For each one, add these two fields:
  tui: false,
  no_tui: false,

Place them alongside the other boolean fields, e.g. after `no_acp_raw: false,`.

There should be 2 occurrences (in `dispatch_run_unknown_backend` and `dispatch_run_without_nl_or_workflow`).

Return a summary with the exact changes.]],
  schema = EDIT_RESULT
})

if not test_fix.ok then
  report({ error = "test fix failed: " .. (test_fix.status or "unknown") })
  return
end

log("test fix: " .. test_fix.output.summary)

-- ── Phase 8: cargo check ────────────────────────────────────────────────

local p8 = phase("verify with cargo check", 1)

local check = agent({
  prompt = [[Run `cargo check` in /Users/apple/dev/maestro to verify everything compiles.

If there are compilation errors:
1. Read the error messages carefully
2. Fix the issues by editing the relevant files
3. Run cargo check again
4. Repeat until cargo check passes OR you hit 5 attempts

Report the final cargo check output and whether it succeeded.

If successful, also run `cargo test --lib` to make sure library tests pass.

Return the full output and success/failure status.]],
  schema = CHECK_RESULT,
  timeout_ms = 180000
})

if not check.ok then
  report({
    error = "verification agent failed: " .. (check.status or "unknown"),
    phase = "verification"
  })
  return
end

-- ── Final Report ────────────────────────────────────────────────────────

local phases_output = {
  { name = "analyze codebase", status = "completed" },
  { name = "add ratatui + crossterm deps", status = cargo_edit.output.changed and "completed" or "skipped" },
  { name = "add --tui / --no-tui flags", status = main_edit.output.changed and "completed" or "skipped" },
  { name = "wire TTY detection + dispatch", status = run_edit.output.changed and "completed" or "skipped" },
  { name = "create run_tui.rs", status = tui_file.output.lines > 0 and "completed" or "failed" },
  { name = "register run_tui module", status = mod_edit.output.changed and "completed" or "skipped" },
  { name = "fix test RunArgs constructions", status = test_fix.output.changed and "completed" or "skipped" },
  { name = "verify with cargo check", status = check.output.success and "completed" or "failed" },
}

report({
  task = "Implement real-time ratatui TUI for `maestro run`",
  status = check.output.success and "completed" or "needs_fixes",
  cargo_check = check.output.success,
  cargo_output = check.output.output,
  errors = check.output.errors or {},
  phases = phases_output,
  files_changed = {
    "Cargo.toml",
    "src/main.rs",
    "src/commands/run.rs",
    "src/commands/run_tui.rs (new)",
    "src/commands/mod.rs",
  },
  summary = check.output.success
    and "All changes pass cargo check. The TUI is wired in: TTY auto-detection, --tui/--no-tui flags, full ratatui rendering loop with real-time agent progress tracking."
    or "cargo check reported errors. Review the output above and fix remaining issues."
})
```
