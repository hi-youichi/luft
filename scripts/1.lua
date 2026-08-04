-- Maestro optimization orchestrator
-- Goals: concise, readable, performant, better test coverage
-- Targets: unwrap() hygiene, clone reduction, test gaps, dead code cleanup

local ANALYSIS_SCHEMA = {
  type = "object",
  properties = {
    findings = {
      type = "array",
      items = {
        type = "object",
        properties = {
          file = { type = "string" },
          line = { type = "integer" },
          issue = { type = "string" },
          severity = { type = "string", enum = { "P0", "P1", "P2", "P3" } }
        },
        required = { "file", "line", "issue", "severity" }
      }
    },
    summary = { type = "string" }
  },
  required = { "findings", "summary" }
}

local OPT_SCHEMA = {
  type = "object",
  properties = {
    file = { type = "string" },
    changes = { type = "array", items = { type = "string" } },
    success = { type = "boolean" },
    build_ok = { type = "boolean" },
    summary = { type = "string" }
  },
  required = { "file", "success", "build_ok", "summary" }
}

local VERIFY_SCHEMA = {
  type = "object",
  properties = {
    tests_pass = { type = "boolean" },
    clippy_ok = { type = "boolean" },
    fmt_ok = { type = "boolean" },
    test_count = { type = "integer" },
    test_summary = { type = "string" },
    clippy_warnings = { type = "array", items = { type = "string" } },
    summary = { type = "string" }
  },
  required = { "tests_pass", "clippy_ok", "summary" }
}

local REPO = "/Users/apple/dev/maestro"

phase("analyze", 3)

local analysis_jobs = parallel({
  { topic = "unwrap and clone issues" },
  { topic = "test coverage gaps" },
  { topic = "long file refactoring targets" }
}, function(job)
  local prompt
  if job.topic == "unwrap and clone issues" then
    prompt = "Read these files and find every production unwrap() call (not inside #[cfg(test)]) and every unnecessary .clone(): " .. REPO .. "/src/mcp.rs, " .. REPO .. "/src/runtime/converge.rs, " .. REPO .. "/src/core/state.rs, " .. REPO .. "/src/core/journal.rs, " .. REPO .. "/src/adapters/acp_adapter.rs. For each finding return file, exact line number, the code snippet, and a severity (P0=panic risk, P1=performance, P2=smell, P3=minor)."
  elseif job.topic == "test coverage gaps" then
    prompt = "Check which modules in " .. REPO .. "/src/ lack #[cfg(test)] mod tests blocks. Read: src/runtime/sandbox.rs, src/core/contract/backend.rs, src/core/contract/event.rs, src/core/contract/finding.rs, src/core/scheduler/config.rs, src/core/scheduler/error.rs, src/core/mock_backend.rs, src/storage/error.rs. For files missing tests, recommend 3-5 specific test cases. Return as structured findings."
  else
    prompt = "Analyze the 3 largest files for refactoring: " .. REPO .. "/src/runtime/converge.rs (1554 lines), " .. REPO .. "/src/storage/writer.rs (1250 lines), " .. REPO .. "/src/service/run.rs (1133 lines). For each: find every function > 80 lines, identify duplicated code blocks (>5 lines repeated 2+ times), and count max nesting depth. Suggest specific extractions."
  end
  return { prompt = prompt, schema = ANALYSIS_SCHEMA }
end)

local all_findings = {}
for _, r in ipairs(analysis_jobs) do
  if r.ok and r.output and r.output.findings then
    for _, f in ipairs(r.output.findings) do
      table.insert(all_findings, f)
    end
  end
end

log(string.format("Found %d issues across all categories", #all_findings), "info")

phase("optimize", 5)

local fix_targets = {
  { file = "mcp.rs",        path = "src/mcp.rs" },
  { file = "converge.rs",   path = "src/runtime/converge.rs" },
  { file = "state.rs",      path = "src/core/state.rs" },
  { file = "phases.rs",     path = "src/service/phases.rs" },
  { file = "sandbox.rs",    path = "src/runtime/sandbox.rs" }
}

local opt_results = parallel(fix_targets, function(t)
  local prompt
  if t.file == "mcp.rs" then
    prompt = "Edit " .. REPO .. "/src/mcp.rs to fix 2 issues:\n1. Replace ALL .unwrap() calls on RwLock with .expect(\"... lock poisoned\") at lines 47,53,58,64,70,75,80,86,92.\n2. Fix get_findings() line 52-54 get_artifacts() line 69-71 get_logs() line 74-76: currently self.X.read().unwrap().clone() clones the entire Vec on every read. Change to read under lock and clone only the data needed, or keep the read lock scope short and return a Vec from the locked data.\nAfter editing, run 'cargo build 2>&1 | tail -20' and report if it compiles."
  elseif t.file == "converge.rs" then
    prompt = "Edit " .. REPO .. "/src/runtime/converge.rs to remove 2 unnecessary clones:\n1. Line 127: change 'let round_input = state.items.clone();' to just use '&state.items' directly since generate_findings takes &[Value].\n2. Line 284: change 'findings.iter().map(|f| (f.clone(), 0)).collect()' to use indices or references instead of cloning every Finding. For example: let mut approvals = vec![0usize; findings.len()]; and use index-based access.\nAlso fix the double model.clone() on lines 134 and 156 by cloning once before the loop.\nAfter editing, run 'cargo build 2>&1 | tail -20' and report if it compiles."
  elseif t.file == "state.rs" then
    prompt = "Edit " .. REPO .. "/src/core/state.rs: replace every .unwrap() on checkpoint.read() and checkpoint.write() with .expect(\"checkpoint lock poisoned\"). Affected lines include: 170, 211, 214, 250, 253, 278, 281, 292, 306, 375, 383, 389, 398, 427, 437, 442. Use a consistent pattern like:\nself.checkpoint.read().expect(\"checkpoint lock\")\nself.checkpoint.write().expect(\"checkpoint lock\")\nAfter editing, run 'cargo build 2>&1 | tail -20' and report if it compiles."
  elseif t.file == "phases.rs" then
    prompt = "Edit " .. REPO .. "/src/service/phases.rs:\n1. Lines 363-367: PhaseDoneInfo has #[allow(dead_code)] on ok and failed fields. If these fields are not read anywhere, remove them and the annotation. If they might be needed later, keep them but narrow the allow to just the fields.\n2. Line 444: replace Utc.timestamp_opt(secs, 0).unwrap() with Utc.timestamp_opt(secs, 0).single().expect(\"valid timestamp\")\nAfter editing, run 'cargo build 2>&1 | tail -20' and report if it compiles."
  elseif t.file == "sandbox.rs" then
    prompt = "Add a #[cfg(test)] mod tests { ... } block to " .. REPO .. "/src/runtime/sandbox.rs. The block should go at the very end of the file (after line 132). Include these tests using super::*:\n1. test_apply_sandbox_blocks_forbidden_globals: creates a Lua VM, calls apply_sandbox, verifies io/os/require/debug globals are nil.\n2. test_apply_sandbox_allows_sdk_globals: verifies agent/report/json globals are still accessible.\n3. test_validate_script_valid_syntax: verifies validate_script accepts 'return 1 + 1'.\n4. test_validate_script_invalid_syntax: verifies validate_script rejects 'syntax ??? error'.\n5. test_validate_script_empty: verifies empty string is valid.\nAfter editing, run 'cargo build 2>&1 | tail -20' and report if it compiles."
  else
    prompt = "no changes needed"
  end
  return { prompt = prompt, schema = OPT_SCHEMA }
end)

local opt_ok = 0
local opt_build_ok = 0
for _, r in ipairs(opt_results) do
  if r.ok and r.output then
    if r.output.success then opt_ok = opt_ok + 1 end
    if r.output.build_ok then opt_build_ok = opt_build_ok + 1 end
  end
end

log(string.format("optimized %d/%d files, %d pass build", opt_ok, #fix_targets, opt_build_ok), "info")

phase("verify", 1)

local verify = agent({
  prompt = "Run commands in " .. REPO .. " and report results:\n1. 'cargo test 2>&1' - record pass/fail and summary line\n2. 'cargo clippy --all-targets 2>&1' - count warnings if any\n3. 'cargo fmt --check 2>&1' - check formatting\nReturn structured output with booleans tests_pass, clippy_ok, fmt_ok and summary string.",
  schema = VERIFY_SCHEMA
})

report({
  status = "ok",
  goals = { "concise", "readable", "performant", "higher test coverage" },
  analysis = { total_findings = #all_findings, findings = all_findings },
  optimization = {
    files_targeted = #fix_targets,
    files_ok = opt_ok,
    files_pass_build = opt_build_ok,
    per_file = opt_results
  },
  verification = verify.ok and verify.output or {
    tests_pass = false, clippy_ok = false, summary = "verification agent failed"
  }
})