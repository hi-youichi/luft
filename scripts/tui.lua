local SCHEMA_PLAN = {
  type = "object",
  properties = {
    summary = { type = "string" },
    steps = { type = "array", items = { type = "object", properties = {
      step = { type = "integer" }, file = { type = "string" },
      action = { type = "string" },
      description = { type = "string" },
      instructions = { type = "string" }
    }, required = { "step", "file", "action", "description", "instructions" } } }
  },
  required = { "summary", "steps" }
}

local SCHEMA_RESULT = {
  type = "object",
  properties = {
    success = { type = "boolean" },
    summary = { type = "string" },
    files_changed = { type = "array", items = { type = "string" } }
  },
  required = { "success", "summary", "files_changed" }
}

local SCHEMA_VERIFY = {
  type = "object",
  properties = {
    success = { type = "boolean" },
    output = { type = "string" },
    errors = { type = "array", items = { type = "string" } }
  },
  required = { "success" }
}

phase("plan")

local plan = agent({
  prompt = [==[
Read these files from /Users/apple/dev/maestro:
- docs/design/run-tui.md
- src/commands/run.rs
- src/main.rs
- Cargo.toml
- src/commands/mod.rs
- src/core/contract/event.rs
- src/service/phases.rs
- src/core/state.rs

Then produce a detailed implementation plan for the run TUI with exactly these steps:

Step 1: Add ratatui + crossterm dependencies to Cargo.toml
Step 2: Add --tui/--no-tui flags to RunArgs in main.rs
Step 3: Add pub mod run_tui to commands/mod.rs
Step 4: Create src/commands/run_tui.rs with full TUI code (TuiApp, render, event loop, run_tui function)
Step 5: Modify src/commands/run.rs to add TTY detection and TUI branching

For each step include: which file, what action (create/modify), a description, and exact instructions with code snippets showing exactly what to write or change.
]==],
  schema = SCHEMA_PLAN
})
if not plan.ok then
  report({ error = "plan failed: " .. (plan.status or "unknown") })
  return
end

local steps = plan.output.steps
if not steps or #steps == 0 then
  report({ error = "plan returned no steps" })
  return
end

log("plan: " .. (plan.output.summary or ""))

phase("implement")

local impl_results = {}
for _, s in ipairs(steps) do
  local instructions = s.instructions
  if not instructions or instructions == "" then
    instructions = s.description
  end
  local prompt_text = "Implement step " .. s.step .. ": " .. s.file .. " (" .. s.action .. ")\n"
  prompt_text = prompt_text .. s.description .. "\n\n"
  prompt_text = prompt_text .. "Instructions:\n" .. instructions
  prompt_text = prompt_text .. "\n\nRead the current file, make the changes, verify they are correct, and report what you did."

  local r = agent({ prompt = prompt_text, schema = SCHEMA_RESULT })
  table.insert(impl_results, { step = s.step, file = s.file, result = r })
  if not r.ok then
    log("step " .. s.step .. " agent error: " .. (r.status or "unknown"), "warn")
  elseif not r.output.success then
    log("step " .. s.step .. " failed: " .. r.output.summary, "warn")
  else
    log("step " .. s.step .. " done: " .. r.output.summary)
  end
end

phase("verify")

local verify = agent({
  prompt = "Run `cargo check` in /Users/apple/dev/maestro. Show the full output. Return success=true if it passes, or list errors if it fails.",
  schema = SCHEMA_VERIFY
})

if verify.ok and verify.output then
  if verify.output.success then
    log("verification passed")
  else
    log("verification failed, attempting auto-fix", "warn")
    local errors_text = ""
    if verify.output.errors and #verify.output.errors > 0 then
      errors_text = table.concat(verify.output.errors, "\n")
    end
    local out_text = verify.output.output or ""
    local fix = agent({
      prompt = "cargo check failed in /Users/apple/dev/maestro.\n\nOutput: " .. out_text .. "\n\nErrors:\n" .. errors_text .. "\n\nAnalyze each error, read the relevant files, fix all issues, then run cargo check again. Repeat until it passes.",
      schema = SCHEMA_VERIFY
    })
    if fix.ok and fix.output.success then
      log("fix succeeded")
    else
      log("fix failed", "error")
    end
  end
end

report({
  status = "implemented",
  summary = plan.output.summary,
  plan_steps = #steps,
  implementation = impl_results,
  verification = verify.ok and verify.output or { success = false }
})