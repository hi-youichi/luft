-- Maestro orchestration script: optimize code (concise, readable, performant, tested)
-- Orchestrates LLM subagents to analyze, refactor, and expand test coverage.
-- Each agent has full tool access (read/write/edit/bash); the script only coordinates.

local TASK_SCHEMA = {
  type = "object",
  properties = {
    file = { type = "string" },
    changes = { type = "array", items = { type = "string" } },
    summary = { type = "string" },
    tests_added = { type = "integer" },
    tests_passed = { type = "boolean" }
  },
  required = { "file", "changes", "summary", "tests_added", "tests_passed" }
}

local ANALYSIS_SCHEMA = {
  type = "object",
  properties = {
    priority_issues = { type = "array", items = {
      type = "object",
      properties = {
        file = { type = "string" },
        issue = { type = "string" },
        line = { type = "integer" },
        severity = { type = "string", enum = { "high", "medium", "low" } }
      },
      required = { "file", "issue", "severity" }
    } },
    summary = { type = "string" }
  },
  required = { "priority_issues", "summary" }
}

local VERIFY_SCHEMA = {
  type = "object",
  properties = {
    compile_ok = { type = "boolean" },
    all_tests_pass = { type = "boolean" },
    test_count = { type = "integer" },
    errors = { type = "array", items = { type = "string" } },
    summary = { type = "string" }
  },
  required = { "compile_ok", "all_tests_pass", "summary" }
}

-- ============================================================================
-- Budget
-- ============================================================================
budget(600000, 100)

-- ============================================================================
-- Phase 1: Analysis
-- ============================================================================
phase("analysis", 1)
log("Analyzing codebase for optimization opportunities")

local analysis = agent({
  prompt = [[You are a Rust code quality auditor. Analyze the Maestro codebase at /Users/apple/dev/maestro and identify the top 10 optimization opportunities.

Read the following files and return structured findings:
1. src/core/state.rs (552 lines) - look for code duplication between init_run/init_run_with_meta, cancel() anti-pattern (write→drop→read→write), duplicate update_from_event on both RunCheckpoint and RunStore, repeated magic strings
2. src/runtime/sandbox.rs (132 lines) - look for missing tests for apply_sandbox, validate_script, register_sdk
3. src/commands/run_tui.rs (596 lines) - look for minimal test coverage (only 3 tests), untested handle_event edge cases, untested scroll_up/scroll_down
4. src/commands/run.rs (541 lines) - look for should_use_tui with process::exit (hard exit), write_report pattern
5. src/runtime/pipeline.rs (596 lines) - look for unused constructs (converge, PipelineError variants)
6. src/runtime/sdk/convert.rs (181 lines) - look for unwrap_or_default() that silently swallows errors

For HIGH priority issues, provide the exact file path, line numbers, and suggested fix.
For MEDIUM/LOW issues, just note the file and general area.

Return priority_issues as an array sorted by severity (high first).]],
  schema = ANALYSIS_SCHEMA
})

if not analysis.ok then
  log("Analysis failed: " .. (analysis.status or "unknown"), "error")
  report({ error = "analysis failed", status = analysis.status })
  return
end

log("Analysis found " .. #analysis.output.priority_issues .. " issues: " .. analysis.output.summary)

-- ============================================================================
-- Phase 2: Core Optimizations (sequential — each depends on the file state)
-- ============================================================================
phase("optimizations", 4)

-- ── Task 2a: Deduplicate init_run / init_run_with_meta in state.rs ──
log("Task 2a: Deduplicate init_run/init_run_with_meta")
local fix_a = agent({
  prompt = [[Edit /Users/apple/dev/maestro/src/core/state.rs to deduplicate init_run and init_run_with_meta.

The two methods are nearly identical (lines 185-257). Only difference is init_run_with_meta has a workflow_meta field.

Change init_run (line 185-218) to DELEGATE to init_run_with_meta instead of duplicating all the logic:
```rust
pub fn init_run(&self, run_id: RunId, task: &str) -> Result<(), std::io::Error> {
    let meta = crate::planner::PlanMeta { label: task.to_string(), phases: vec![] };
    self.init_run_with_meta(run_id, task, meta)
}
```
Wait — PlanMeta might not have those exact fields. Instead, use a more careful approach.

Read the actual file first, then make BOTH changes:
1. Keep init_run as-is but have its constructor logic call init_run_with_meta(None)
2. Actually, init_run creates WITHOUT meta and init_run_with_meta creates WITH meta. The simplest fix:
   - Remove the duplicate code from init_run
   - Make init_run call init_run_with_meta with workflow_meta=None
   - But PlanMeta is not Option<PlanMeta> in the signature... 

OK read the file carefully. The better approach:
1. Extract the common save_checkpoint + open events file + set lock pattern into a shared private helper fn
2. Have both init_run and init_run_with_meta call that helper

Read src/core/state.rs, then:
- Add a private method `fn init_common(&self, checkpoint: RunCheckpoint) -> Result<(), std::io::Error>` that contains the shared code (save_checkpoint, open events file, set locks)
- Refactor init_run to create the checkpoint and call init_common
- Refactor init_run_with_meta to create the checkpoint and call init_common

Also simplify the cancel() method (lines 435-451):
- It currently acquires write lock, drops it, acquires read lock — unnecessary
- Just hold the write lock through the entire operation

After making changes, run: cargo build 2>&1 | tail -20
If it compiles, run: cargo test --package maestro -- core::state 2>&1 | tail -30
Return the result. Do NOT use `cd` — use the workdir parameter for bash commands.]],
  schema = TASK_SCHEMA
})

if not fix_a.ok then
  log("Task 2a failed: " .. (fix_a.status or "unknown"), "error")
  report({ error = "optimization 2a failed", status = fix_a.status })
  return
end
log("Task 2a: " .. fix_a.output.summary)

-- ── Task 2b: Deduplicate event handling in state.rs ──
log("Task 2b: Deduplicate event handling (update_from_event)")
local fix_b = agent({
  prompt = [[Edit /Users/apple/dev/maestro/src/core/state.rs to eliminate the duplicated event-matching logic.

Currently there are TWO update_from_event methods:
1. RunCheckpoint::update_from_event (struct method, lines 42-86) — updates checkpoint in-memory
2. RunStore::update_from_event (private method, lines 305-358) — has IDENTICAL event matching, but also persists to disk

The fix:
- Change RunStore::update_from_event to call `checkpoint.update_from_event(event)` instead of duplicating all the match arms
- Then just persist to disk

Specifically, replace the large match block in RunStore::update_from_event (lines 308-350) with just:
```rust
checkpoint.update_from_event(event);
```
Since RunStore already holds the write lock on checkpoint, calling the struct method modifies it in-place.

After making changes, run: cargo build 2>&1 | tail -20
Then: cargo test --package maestro -- core::state 2>&1 | tail -30
Return the result.]],
  schema = TASK_SCHEMA
})

if not fix_b.ok then
  log("Task 2b failed: " .. (fix_b.status or "unknown"), "error")
  report({ error = "optimization 2b failed", status = fix_b.status })
  return
end
log("Task 2b: " .. fix_b.output.summary)

-- ── Task 2c: Extract repeated path strings as constants ──
log("Task 2c: Extract magic strings as constants")
local fix_c = agent({
  prompt = [[Edit /Users/apple/dev/maestro/src/core/state.rs to extract repeated path strings as module-level constants.

The strings "checkpoint.json" and "events.jsonl" appear 7+ times throughout the file:
- "checkpoint.json" at lines 138, 176, 262, 268, 362, 370, 444
- "events.jsonl" at lines 205, 244, 273, 407

Add at the top of the file (after the imports, before RunCheckpoint):
```rust
const CHECKPOINT_FILE: &str = "checkpoint.json";
const EVENTS_FILE: &str = "events.jsonl";
```

Then replace all occurrences of "checkpoint.json" with CHECKPOINT_FILE and "events.jsonl" with EVENTS_FILE.

Similarly, extract current_timestamp's SystemTime::now().duration_since(UNIX_EPOCH) pattern is repeated inside RunStore::new() at line 140 and elsewhere - actually it's only in current_timestamp(), so that's fine.

After making changes, run: cargo build 2>&1 | tail -20
Then: cargo test --package maestro -- core::state 2>&1 | tail -30
Return the result.]],
  schema = TASK_SCHEMA
})

if not fix_c.ok then
  log("Task 2c failed: " .. (fix_c.status or "unknown"), "error")
  report({ error = "optimization 2c failed", status = fix_c.status })
  return
end
log("Task 2c: " .. fix_c.output.summary)

-- ── Task 2d: Minor perf / readability fixes ──
log("Task 2d: Minor readability improvements in run.rs")
local fix_d = agent({
  prompt = [[Make minor improvements to /Users/apple/dev/maestro/src/commands/run.rs.

Read the file first, then:

1. Simplify should_use_tui (lines 137-153): Replace the std::process::exit(1) calls with returning an error. Actually it can't easily return Result since the caller expects bool. Instead, just improve the TTY check: use the existing `is_terminal()` more cleanly.

Actually the simplest improvement: the write_report function (line 118-134) has this pattern:
```rust
if let Some(parent) = path.parent() {
    if !parent.as_os_str().is_empty() {
        std::fs::create_dir_all(parent)?;
    }
}
```
Simplify to:
```rust
if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
    std::fs::create_dir_all(parent)?;
}
```

Make ONLY this change. Then:
cargo build 2>&1 | tail -10
cargo test --package maestro -- commands::run 2>&1 | tail -30
Return the result.]],
  schema = TASK_SCHEMA
})

if not fix_d.ok then
  log("Task 2d failed: " .. (fix_d.status or "unknown"), "error")
  report({ error = "optimization 2d failed", status = fix_d.status })
  return
end
log("Task 2d: " .. fix_d.output.summary)

-- ============================================================================
-- Phase 3: Test coverage
-- ============================================================================
phase("test_coverage", 4)

-- ── Task 3a: Comprehensive tests for state.rs ──
log("Task 3a: Add comprehensive tests for state.rs")
local test_a = agent({
  prompt = [[Add comprehensive tests to /Users/apple/dev/maestro/src/core/state.rs.

Read the file first. The existing tests (lines 511-552) only cover 3 methods: init_run, open_run, can_resume.

Add tests for ALL of these untested methods:
- append_event (with RunStarted event, then read back from events.jsonl)
- update_from_event (via RunStore — check that a RunDone event updates the checkpoint status)
- upsert_agent_result
- get_agent_results
- get_findings
- cancel
- save_checkpoint
- get_event_log
- list_runs (from global management)
- init_run_with_meta
- get_run_store

Use tempfile::tempdir() for each test. Follow the existing test patterns.

IMPORTANT: Each test must be INDEPENDENT and use its own temp dir.

Make sure the tests compile and pass:
cargo build 2>&1 | tail -20
cargo test --package maestro -- core::state 2>&1 | tail -50

Return the changes_summary with test count and pass/fail status.]],
  schema = TASK_SCHEMA
})

if not test_a.ok then
  log("Task 3a failed: " .. (test_a.status or "unknown"), "error")
  report({ error = "test coverage 3a failed", status = test_a.status })
  return
end
log("Task 3a: " .. test_a.output.summary .. " (added " .. test_a.output.tests_added .. " tests)")

-- ── Task 3b: Tests for sandbox.rs ──
log("Task 3b: Add tests for sandbox.rs")
local test_b = agent({
  prompt = [[Add comprehensive unit tests to /Users/apple/dev/maestro/src/runtime/sandbox.rs.

Read the file first. There are ZERO tests currently.

Add a #[cfg(test)] mod tests block with tests for:
1. apply_sandbox — verify that io, os, debug, package, require, loadfile, dofile are all nil after sandbox is applied
2. validate_script — valid Lua script returns Ok
3. validate_script — invalid Lua script returns Err
4. validate_script — empty script is valid
5. register_sdk — verify that agent, parallel, pipeline, phase, log, budget, report, json.encode, json.decode, workflow globals exist after registration (use Lua::new() + mock SdkContext)

For test 1, apply_sandbox is private. Test it indirectly by creating a Runtime with mock dependencies then checking the globals.

Actually the simplest approach: since apply_sandbox and register_sdk are private, you can:
1. test validate_script directly (it's pub)
2. for sandbox, create a Lua instance, call apply_sandbox directly within a test — but it's fn, not pub fn. 
   Instead, add `#[cfg(test)] pub(crate) fn` wrappers, or just test Runtime::execute (which is public).

Better approach: 
- Test validate_script directly (returns Result)
- Test that Runtime::new() + execute() blocks io/os access:
  Create a Runtime (need scheduler, run_ctx, etc.), run a script that tries os.execute("echo hi"), verify it returns an error

Actually, Runtime::new requires Arc<Scheduler> and RunContext which is complex to set up. The simplest viable tests:
1. validate_script with valid Lua
2. validate_script with syntax error  
3. validate_script with empty string
4. validate_script with sandbox-violating code (io.open) — this would pass syntax validation since sandbox is applied at runtime, not validation

For the sandbox tests, use the pattern from existing tests in the repo:
Look at tests/runtime_e2e.rs for the test setup pattern, then write similar tests.

Read tests/runtime_e2e.rs for the setup pattern, then add tests to sandbox.rs.

After adding tests:
cargo build 2>&1 | tail -20
cargo test --package maestro -- runtime::sandbox 2>&1 | tail -50

Return the result.]],
  schema = TASK_SCHEMA
})

if not test_b.ok then
  log("Task 3b failed: " .. (test_b.status or "unknown"), "error")
  report({ error = "test coverage 3b failed", status = test_b.status })
  return
end
log("Task 3b: " .. test_b.output.summary)

-- ── Task 3c: Tests for run_tui.rs ──
log("Task 3c: Add tests for run_tui.rs")
local test_c = agent({
  prompt = [[Add more tests to /Users/apple/dev/maestro/src/commands/run_tui.rs.

Read the file first. Only 3 tests exist (test_phase_icon_and_style, test_tui_app_new_initial_state, test_tui_app_handle_run_done).

Add tests for:
1. TuiApp::scroll_up — verify offset decreases
2. TuiApp::scroll_down — verify offset increases
3. TuiApp::handle_event with AgentStarted — verify running_agents gets populated
4. TuiApp::handle_event with AgentProgress (Tokens, ToolCall, Message deltas)
5. TuiApp::handle_event with PhaseStarted — verify checkpoint updated
6. TuiApp::handle_event with RunDone with Failed status — verify finished=true
7. render_header — test the header rendering logic indirectly via TuiApp state
8. phase_icon_and_style coverage is already complete

Note: test_tui_app_new_initial_state has a bug at line 563:
```rust
assert_eq!(app.scroll, 0);
```
There is NO `scroll` field on TuiApp! The scrolling is handled by `list_state`. 
Fix this line to: `assert_eq!(app.list_state.offset(), 0);`

Make sure all existing tests still pass after your changes:
cargo test --package maestro -- commands::run_tui 2>&1 | tail -50
Return the result.]],
  schema = TASK_SCHEMA
})

if not test_c.ok then
  log("Task 3c failed: " .. (test_c.status or "unknown"), "error")
  report({ error = "test coverage 3c failed", status = test_c.status })
  return
end
log("Task 3c: " .. test_c.output.summary)

-- ── Task 3d: Add e2e test for sandbox enforcement ──
log("Task 3d: Add e2e sandbox enforcement test")
local test_d = agent({
  prompt = [[Add an end-to-end integration test to /Users/apple/dev/maestro/tests/runtime_e2e.rs.

Read the file first. Add a test that verifies the sandbox blocks I/O operations:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_blocks_io_operations() {
    // Script that tries to use I/O — should fail at runtime
    let script = r#"
        io.open("/tmp/test.txt")
        report({ reached = true })
    "#;

    let backend = ok_backend();
    let registry = BackendRegistry::new().with(backend as Arc<dyn AgentBackend>);
    let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
    let run_id = uuid::Uuid::now_v7();
    let (tx, _rx) = tokio::sync::broadcast::channel(64);
    let run_ctx = RunContext { run_id, cancel: CancellationToken::new(), events: tx };
    scheduler.init_run_with(run_id, run_ctx.events.clone());
    let handle = tokio::runtime::Handle::current();
    let rt = Runtime::new(scheduler, run_ctx, serde_json::json!({}), ExecLimits::default(), None, handle)
        .expect("runtime init");

    let res = tokio::task::spawn_blocking(move || rt.execute(script)).await.unwrap();
    assert!(res.is_err(), "sandbox should block io.open");
    let err = res.unwrap_err().to_string();
    assert!(err.contains("io") || err.contains("sandbox") || err.contains("nil"),
        "error should mention io/sandbox: {}", err);
}
```

Wait, the io.open call would likely result in a Lua error like "attempt to index a nil value (global 'io')", but this gets wrapped in mlua::Error. The Runtime::execute returns Result<Value, ScriptError>, and ScriptError has a From<mlua::Error>.

Actually for a cleaner test, also add:
```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_blocks_os_execute() {
    let script = r#"os.execute("echo hi")"#;
    // ... same setup as above ...
    let res = ...;
    assert!(res.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]  
async fn sandbox_blocks_require() {
    let script = r#"require("some_module")"#;
    // ... same setup ...
    let res = ...;
    assert!(res.is_err());
}
```

Use the ok_backend() helper that's already in the file.

After adding tests:
cargo build --test runtime_e2e 2>&1 | tail -20
cargo test --test runtime_e2e 2>&1 | tail -50

Return the result with summary of what was added.]],
  schema = TASK_SCHEMA
})

if not test_d.ok then
  log("Task 3d failed: " .. (test_d.status or "unknown"), "error")
  report({ error = "test coverage 3d failed", status = test_d.status })
  return
end
log("Task 3d: " .. test_d.output.summary)

-- ============================================================================
-- Phase 4: Full verification
-- ============================================================================
phase("verification", 1)
log("Running full test suite")

local verify = agent({
  prompt = [[Run the full test suite for the Maestro project and report results.

Execute:
cargo test 2>&1

Wait for it to complete (timeout 300s). Capture the FULL output. Report:
- Whether compilation succeeded
- Total test count
- Number passed vs failed
- List any failing test names
- Summary line

If ANY tests failed, include the failure output in the errors array.]],
  schema = VERIFY_SCHEMA,
  timeout_ms = 300000
})

if not verify.ok then
  log("Verification error: " .. (verify.status or "unknown"), "error")
  -- Still report partial results if we have them
  local partial = verify.output or {}
  report({
    phase = "verification",
    error = verify.status,
    partial_compile_ok = partial.compile_ok,
    partial_test_count = partial.test_count,
    summary = "Verification agent encountered an error; results may be incomplete"
  })
  return
end

-- ============================================================================
-- Final report
-- ============================================================================
report({
  task = "Code Optimization: concise, readable, performant, tested",
  analysis = {
    issues_found = #analysis.output.priority_issues,
    summary = analysis.output.summary
  },
  optimizations = {
    dedup_init_run = fix_a.output.summary,
    dedup_event_handling = fix_b.output.summary,
    extract_constants = fix_c.output.summary,
    run_rs_simplify = fix_d.output.summary,
    files_changed = {
      fix_a.output.file,
      fix_b.output.file,
      fix_c.output.file,
      fix_d.output.file
    }
  },
  test_coverage = {
    state_rs = { tests_added = test_a.output.tests_added, summary = test_a.output.summary },
    sandbox_rs = test_b.output.summary,
    run_tui_rs = test_c.output.summary,
    e2e_sandbox = test_d.output.summary
  },
  verification = {
    compile_ok = verify.output.compile_ok,
    all_tests_pass = verify.output.all_tests_pass,
    test_count = verify.output.test_count,
    errors = verify.output.errors or {},
    summary = verify.output.summary
  }
})
