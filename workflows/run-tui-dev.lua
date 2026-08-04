local GAP_SCHEMA = {
  type = "object",
  properties = {
    missing_items = {
      type = "array",
      items = {
        type = "object",
        properties = {
          step = { type = "integer" },
          file = { type = "string" },
          description = { type = "string" },
          details = { type = "string" },
        },
        required = { "step", "file", "description" },
      },
    },
    summary = { type = "string" },
  },
  required = { "missing_items", "summary" },
}

local FILE_SCHEMA = {
  type = "object",
  properties = { content = { type = "string" }, explanation = { type = "string" } },
  required = { "content" },
}

local MODIFY_SCHEMA = {
  type = "object",
  properties = { modified_content = { type = "string" }, changes = { type = "string" } },
  required = { "modified_content", "changes" },
}

local VERIFY_SCHEMA = {
  type = "object",
  properties = {
    success = { type = "boolean" },
    output = { type = "string" },
    errors = { type = "array", items = { type = "string" } },
  },
  required = { "success", "output" },
}

-- FIXED_GAP: the prompt for reading phases.rs mentions the detail field type
-- FIXED_SYNTAX: removed all gsub calls, all long strings use [==[ ... ]==]

phase("analyze-design", 1)

local design = agent({
  prompt = [==[
Read the design document at /Users/apple/dev/maestro/docs/design/run-tui.md

Extract all implementation steps and requirements. Pay special attention to:
1. Required dependencies (ratatui, crossterm)
2. CLI flags (--tui, --no-tui) 
3. TuiApp struct fields
4. Event loop structure
5. Render function layout
6. Keyboard interactions
7. Error handling and fallback behavior
8. The 6 implementation steps from section 9

Return a structured list of what needs to be implemented.
]==],
  schema = GAP_SCHEMA,
})

if not design.ok then
  report({ error = "design analysis failed: " .. (design.status or "unknown") })
  return
end

phase("analyze-codebase", 1)

local codebase = agent({
  prompt = [==[
Read these files and return their COMPLETE contents verbatim (every line):

1. /Users/apple/dev/maestro/src/commands/run.rs
2. /Users/apple/dev/maestro/src/main.rs
3. /Users/apple/dev/maestro/Cargo.toml
4. /Users/apple/dev/maestro/src/commands/mod.rs
5. /Users/apple/dev/maestro/src/service/phases.rs

Return a JSON object with keys "run_rs", "main_rs", "cargo_toml", "mod_rs", "phases_rs"
and each value being the complete file content as a string.
]==],
  schema = {
    type = "object",
    properties = {
      run_rs = { type = "string" },
      main_rs = { type = "string" },
      cargo_toml = { type = "string" },
      mod_rs = { type = "string" },
      phases_rs = { type = "string" },
    },
    required = { "run_rs", "main_rs", "cargo_toml", "mod_rs", "phases_rs" },
  },
})

if not codebase.ok then
  report({ error = "codebase analysis failed: " .. (codebase.status or "unknown") })
  return
end

phase("implement-deps", 1)

local cargo_modified = agent({
  prompt = [==[
Modify the Cargo.toml at /Users/apple/dev/maestro/Cargo.toml to add ratatui and crossterm dependencies.

Current content:
]==] .. codebase.output.cargo_toml .. [==[
  
Requirements:
1. Add ratatui 0.29 with features = ["all-widgets"] in [dependencies]
2. Add crossterm 0.28 in [dependencies]
3. Keep alphabetical order with other deps

Return ONLY the complete modified file content.
]==],
  schema = MODIFY_SCHEMA,
})

if not cargo_modified.ok then
  report({ error = "modify cargo failed: " .. (cargo_modified.status or "unknown") })
  return
end

-- Write Cargo.toml
local w1 = agent({
  prompt = [==[
Write the following content to /Users/apple/dev/maestro/Cargo.toml. Overwrite the file.

Content:
]==] .. cargo_modified.output.modified_content .. [==[
]==],
  schema = FILE_SCHEMA,
})

if not w1.ok then
  report({ error = "write cargo failed: " .. (w1.status or "unknown") })
  return
end

phase("implement-cli-flags", 1)

local main_modified = agent({
  prompt = [==[
Modify /Users/apple/dev/maestro/src/main.rs to add --tui and --no-tui flags to RunArgs.

Current RunArgs struct content:
]==] .. codebase.output.main_rs .. [==[
  
Requirements:
1. Add this field to the RunArgs struct:
   #[arg(long, help = "Force TUI mode (errors if not a TTY)")]
   tui: bool,
2. Add this field:
   #[arg(long, help = "Force headless/log mode (disable TUI)")]
   no_tui: bool,
3. The flags should NOT be mutually exclusive in clap — handle conflicts in code instead.
   Using `conflicts_with` on optional bools requires Option<bool> which is more complex.
   Since both default to false, just check both at runtime.

Return the COMPLETE modified main.rs file.
]==],
  schema = MODIFY_SCHEMA,
})

if not main_modified.ok then
  report({ error = "modify main.rs failed: " .. (main_modified.status or "unknown") })
  return
end

local w2 = agent({
  prompt = [==[
Write the following content to /Users/apple/dev/maestro/src/main.rs. Overwrite the file.

Content:
]==] .. main_modified.output.modified_content .. [==[
]==],
  schema = FILE_SCHEMA,
})

if not w2.ok then
  report({ error = "write main.rs failed: " .. (w2.status or "unknown") })
  return
end

phase("implement-module-register", 1)

local mod_modified = agent({
  prompt = [==[
Modify /Users/apple/dev/maestro/src/commands/mod.rs to add the run_tui module.

Current content:
]==] .. codebase.output.mod_rs .. [==[
  
Add `pub mod run_tui;` after `pub mod run;` (alphabetical order).

Return the COMPLETE modified file content.
]==],
  schema = MODIFY_SCHEMA,
})

if not mod_modified.ok then
  report({ error = "modify mod.rs failed: " .. (mod_modified.status or "unknown") })
  return
end

local w3 = agent({
  prompt = [==[
Write the following content to /Users/apple/dev/maestro/src/commands/mod.rs. Overwrite the file.

Content:
]==] .. mod_modified.output.modified_content .. [==[
]==],
  schema = FILE_SCHEMA,
})

if not w3.ok then
  report({ error = "write mod.rs failed: " .. (w3.status or "unknown") })
  return
end

phase("implement-tui", 1)

local tui_generated = agent({
  prompt = [==[
Create the file /Users/apple/dev/maestro/src/commands/run_tui.rs

This is a ratatui-based real-time TUI for `maestro run`. Here is the full design:

=== DEPENDENCIES ===
ratatui 0.29 with all-widgets, crossterm 0.28

=== ARCHITECTURE ===
Two threads:
1. tokio worker: svc::execute(&run_ctx, prepared.runtime, script) via tokio::spawn
2. main thread: ratatui event loop — receives events via broadcast::Receiver, renders via frame::render()

=== DATA STRUCTURES ===

```rust
pub struct TuiApp {
    pub checkpoint: RunCheckpoint,
    pub events: Vec<AgentEvent>,
    pub view: PhasesView,
    pub finished: bool,
    pub scroll: usize,
    pub show_agent_detail: bool,
    pub running_agents: HashMap<AgentId, RunningAgentState>,
    rx: broadcast::Receiver<AgentEvent>,
}

pub struct RunningAgentState {
    pub agent_id: AgentId,
    pub phase_id: PhaseId,
    pub tokens: u64,
    pub tool_count: usize,
    pub last_message: String,
}
```

=== handle_event ===
- AgentStarted { agent_id, phase_id, .. } -> insert into running_agents
- AgentProgress { agent_id, delta: ProgressDelta::Tokens { usage } } -> update tokens
- AgentProgress { agent_id, delta: ProgressDelta::ToolCall { .. } } -> increment tool_count
- AgentProgress { agent_id, delta: ProgressDelta::Message { text } } -> update last_message
- AgentDone { agent_id, .. } -> remove from running_agents
- PhaseStarted/PhaseDone/RunDone -> checkpoint.update_from_event(&event)
- All events -> push to self.events

After each event: self.rebuild_view()

=== rebuild_view ===
1. Call build_phases_view(&self.checkpoint, &self.events) to get base view
2. For each running_agent, inject live data into the matching PhaseRow.agents entry
   (find agent by short_id match, update tokens/tool_count/last_message)
   Running agents have status "running" in their PhaseRow — they come from 
   collect_running_agents in service/phases.rs which gives them tokens: None etc.
   Override these with the live RunningAgentState values.

=== render layout ===
frame.render() with vertical Layout:
- [H, 3 lines] Header block: "maestro run — {task}", run_id, status, phase x/y, tokens, elapsed
- [*, fill] Phases list: iterate view.phases, render each phase + agents  
- [B, 1 line] Status bar: keybindings or "Run completed. Exiting in 3s..."

Phase line format (like design doc ascii):
  {icon} Phase {n}/{total}  {label}  {status_text}  {elapsed}

Icons: Pending=○ (gray), Running=◐ (blue), Completed=● (green), Failed=✕ (red)

Agent line format (running):
  └─ {short_id}  running   — tok  tools={n}
     │ {message_preview_60chars}

Agent line format (completed):
  └─ {short_id}  completed  {tokens} tok  findings={n}

=== EVENT LOOP ===
```rust
pub async fn run_tui(
    run_ctx: RunContext,
    prepared: PreparedRun,
    script: String,
    output: Option<PathBuf>,
) -> Result<serde_json::Value> {
    let rx = run_ctx.events.subscribe();
    let mut app = TuiApp::new(checkpoint, rx);  // get checkpoint from somewhere
    
    let mut terminal = setup_terminal()?;
    
    let run_handle = tokio::spawn(async move {
        svc::execute(&run_ctx, prepared.runtime, script).await
    });
    
    loop {
        terminal.draw(|f| render(&app, f))?;
        tokio::select! {
            Ok(true) = poll_keyboard() => {
                match read_key()? {
                    Key::Char('q') | Key::Ctrl('c') => {
                        if confirm_exit()? { break; }
                    }
                    Key::Up => app.scroll_up(),
                    Key::Down => app.scroll_down(),
                    Key::Tab => app.show_agent_detail = !app.show_agent_detail,
                    _ => {}
                }
            }
            ev = app.rx.recv() => {
                match ev {
                    Ok(event) => {
                        app.handle_event(event);
                        if app.finished { break; }
                    }
                    Err(RecvError::Closed) => break,
                    Err(RecvError::Lagged(n)) => { /* warning */ }
                }
            }
        }
    }
    
    // Freeze 3s if finished normally
    if app.finished {
        freeze_for(3000, &mut terminal, &app)?;
    }
    
    restore_terminal(&mut terminal)?;
    let result = run_handle.await??;
    Ok(result)
}
```

=== KEYBOARD ===
- q / Ctrl+C: confirm exit dialog, then cancel run or continue
- Up/Down: scroll
- Tab: toggle agent detail
- r: redraw

Poll keyboard with crossterm::event::poll(Duration::from_millis(100))

=== TERMINAL SETUP ===
```rust
fn setup_terminal() -> Result<Terminal<impl Backend>> {
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    Terminal::new(backend).map_err(|e| anyhow::anyhow!("terminal init: {}", e))
}

fn restore_terminal(terminal: &mut Terminal<impl Backend>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
```

=== confirm_exit ===
Show a prompt: "Abort workflow? (y/n)" at the bottom of the screen
Read a single key: y = true (abort), n = false (continue)
Restore the UI after.

=== IMPORTS ===
Use these imports:
```rust
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use maestro::core::contract::event::{AgentEvent, ProgressDelta};
use maestro::core::contract::ids::{AgentId, PhaseId};
use maestro::core::state::RunCheckpoint;
use maestro::service::phases::{build_phases_view, PhasesView, PhaseRow, PhaseStatus};
use maestro::service::run::{self as svc, PreparedRun};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::line;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use ratatui::Terminal;
use std::collections::HashMap;
use std::io::stdout;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
```

NOTE: The checkpoint is obtained from the journal store. In run_workflow,
after prepare() we can get it with prepared.journal.store().get_checkpoint().

BUT the TUI needs it at construction. The cleanest approach:
- Store the initial RunCheckpoint in TuiApp at construction time
- Pass it from run_workflow via prepared.journal.store().get_checkpoint().unwrap()

=== TuiApp::new ===
```rust
pub fn new(checkpoint: RunCheckpoint, rx: broadcast::Receiver<AgentEvent>) -> Self {
    let mut app = TuiApp {
        checkpoint,
        events: Vec::new(),
        view: build_phases_view(&checkpoint, &[]),
        finished: false,
        scroll: 0,
        show_agent_detail: false,
        running_agents: HashMap::new(),
        rx,
    };
    app
}
```

=== Type for RunCheckpoint ===
Use: maestro::core::state::RunCheckpoint
We get the initial checkpoint from the journal store. In the run_workflow function
we call prepared.journal.store().get_checkpoint() which returns Option<RunCheckpoint>.

=== Cargo.toml note ===
ratatui 0.29 uses "all-widgets" feature and CrosstermBackend.
crossterm 0.28 uses the standard event and terminal modules.

=== FULL IMPLEMENTATION ===
Create a complete, compilable Rust file. Include:
1. All use statements
2. TuiApp struct with all 8 fields  
3. RunningAgentState struct
4. impl TuiApp { new(), handle_event(), rebuild_view(), scroll_up(), scroll_down() }
5. render() function that renders TuiApp with ratatui
6. render_header(), render_phases(), render_status_bar() helpers
7. run_tui() async function
8. setup_terminal() and restore_terminal() functions
9. poll_keyboard() and read_key() functions
10. confirm_exit() async function
11. freeze_for() function for the 3-second post-run freeze
12. #[cfg(test)] module with tests for TuiApp::new, TuiApp::handle_event
    (tests should construct TuiApp directly, no terminal required)

Return ONLY the complete file content, no explanation.
]==],
  schema = FILE_SCHEMA,
})

if not tui_generated.ok then
  report({ error = "generate run_tui.rs failed: " .. (tui_generated.status or "unknown") })
  return
end

local w4 = agent({
  prompt = [==[
Write the following content to /Users/apple/dev/maestro/src/commands/run_tui.rs. Create the file.

Content:
]==] .. tui_generated.output.content .. [==[
]==],
  schema = FILE_SCHEMA,
})

if not w4.ok then
  report({ error = "write run_tui.rs failed: " .. (w4.status or "unknown") })
  return
end

phase("implement-dispatch", 1)

local run_modified = agent({
  prompt = [==[
Modify /Users/apple/dev/maestro/src/commands/run.rs to add TUI dispatch logic.

Current content:
]==] .. codebase.output.run_rs .. [==[
  
Changes needed:

1. Add this import at the top after existing use statements:
   use crate::commands::run_tui;
   (Note: "use crate::commands::run_tui;" as a simple import is sufficient)

2. In the run_workflow() function, AFTER the prepared run is created at line 90
   (after `let prepared = svc::prepare(...)`), REPLACE the current tail that calls
   run_headless with a TTY-detection branch:

   The current end of run_workflow is:
   ```
       run_headless(run_ctx, prepared.runtime, spec.script, args.output, logger).await
   }
   ```

   Replace it with something like:
   ```rust
       let use_tui = if args.tui {
           true
       } else if args.no_tui {
           false
       } else {
           std::io::stdout().is_terminal()
       };

       if use_tui {
           drop(logger);
           let checkpoint = prepared.journal.store().get_checkpoint()
               .ok_or_else(|| anyhow::anyhow!("no checkpoint available"))?;
           
           let mut tui_app = run_tui::TuiApp::new(checkpoint, run_ctx.events.subscribe());
           // Store the TUI app handle for the duration
           
           let result = run_tui::run_tui(
               run_ctx,
               prepared,
               spec.script,
               args.output,
           ).await?;
           println!("{}", serde_json::to_string(&serde_json::json!({
               "type": "report",
               "run_id": run_ctx.run_id.to_string(),
               "report": result,
           }))?);
       } else {
           run_headless(run_ctx, prepared.runtime, spec.script, args.output, logger).await?;
       }
       
       Ok(())
   }
   ```

   Actually, keep it simpler. The run_tui function returns the report value.
   The caller handles output. Let me show the exact signature.

IMPORTANT: The run_tui function signature should be:
```rust
pub async fn run_tui(
    run_ctx: RunContext,
    prepared: PreparedRun,
    script: String,
    output: Option<PathBuf>,
) -> Result<serde_json::Value>
```

The run_tui function internally:
1. Gets checkpoint from prepared.journal.store().get_checkpoint().unwrap()
2. Subscribes to run_ctx.events.subscribe()  
3. Spawns execution via svc::execute(&run_ctx, prepared.runtime, script)
4. Runs the ratatui event loop
5. On RunDone, freezes for 3s, restores terminal
6. Returns the report value

So in run_workflow, just call:
```rust
if use_tui {
    let report_value = run_tui::run_tui(run_ctx, prepared, spec.script, args.output).await?;
    // Print the report (matches current behavior)
    // write_report if output is set
    // Print the JSON report line
} else {
    run_headless(...).await?;
}
```

Create a clean, compilable modification. The existing run_headless function
is called after the TUI check. Make sure the logger is dropped in the TUI path
since the TUI handles its own events.

Return the COMPLETE modified run.rs file content.
]==],
  schema = MODIFY_SCHEMA,
})

if not run_modified.ok then
  report({ error = "modify run.rs failed: " .. (run_modified.status or "unknown") })
  return
end

local w5 = agent({
  prompt = [==[
Write the following content to /Users/apple/dev/maestro/src/commands/run.rs. Overwrite the file.

Content:
]==] .. run_modified.output.modified_content .. [==[
]==],
  schema = FILE_SCHEMA,
})

if not w5.ok then
  report({ error = "write run.rs failed: " .. (w5.status or "unknown") })
  return
end

phase("verify", 1)

log("running cargo check", "info")

local verify = agent({
  prompt = [==[
Run `cargo check 2>&1` in the directory /Users/apple/dev/maestro.

Return the complete output. Use a 120-second timeout for this command.

If the output includes errors, return them verbatim. Also return whether it succeeded.

Return: { "success": true/false, "output": "full output text", "errors": ["error1", "error2"] }
]==],
  schema = VERIFY_SCHEMA,
  timeout_ms = 180000,
})

if not verify.ok then
  report({ error = "verify failed: " .. (verify.status or "unknown") })
  return
end

if not verify.output.success then
  log("cargo check failed, fixing errors", "warn")

  phase("fix-errors", 1)

  local fix = agent({
    prompt = [==[
cargo check failed with errors. Fix them all.

Output:
]==] .. verify.output.output .. [==[
  
For each error:
1. Read the relevant source file to understand the context
2. Fix the error using your Edit tool
3. Run `cargo check 2>&1` again
4. Repeat until cargo check passes

Common issues to check:
- run_tui.rs: missing imports, wrong types, API mismatches
- run.rs: incorrect function call signatures, missing imports
- main.rs: RunArgs struct fields, dispatch code
- mod.rs: module declaration order

Work through errors one at a time. Return the final cargo check output.

Return: { "success": true/false, "output": "final cargo check output", "errors": [] }
]==],
    schema = VERIFY_SCHEMA,
    timeout_ms = 300000,
  })

  if not fix.ok then
    report({
      status = "fix_failed",
      error = fix.status,
      initial_errors = verify.output.errors,
    })
    return
  end

  report({
    status = fix.output.success and "completed_with_fixes" or "completed_with_errors",
    cargo_output = fix.output.output,
    files_created = { "src/commands/run_tui.rs" },
    files_modified = {
      "Cargo.toml",
      "src/main.rs",
      "src/commands/mod.rs",
      "src/commands/run.rs",
    },
    errors = fix.output.errors,
  })
  return
end

report({
  status = "completed",
  cargo_output = verify.output.output,
  files_created = { "src/commands/run_tui.rs" },
  files_modified = {
    "Cargo.toml",
    "src/main.rs",
    "src/commands/mod.rs",
    "src/commands/run.rs",
  },
})
