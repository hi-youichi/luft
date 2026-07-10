--------------------------------------------
-- Goal:  Add unit tests to all source files currently lacking them
-- Arch:
--   discover ==> [untested_files[]]
--     (for each file_group)
--       analyze ==> write_tests --> [test_code]
--     <== verify
--   <== report
-- Flow:  file_list -> parallel_groups -> test_code -> verify -> report
--------------------------------------------
meta = {
    reasoning = "Parallel fan-out: each agent reads a source file and appends comprehensive #[cfg(test)] tests, then a verify agent runs cargo test to confirm.",
    phases = {
        {
            label = "write_tests",
            detail = "Write unit tests for each untested file in parallel",
            agents = 8,
            depends_on = {}
        },
        {
            label = "verify",
            detail = "Run cargo test and fix any compilation failures",
            agents = 1,
            depends_on = { 0 }
        },
        {
            label = "report",
            detail = "Summarize coverage improvement results",
            agents = 1,
            depends_on = { 1 }
        },
    },
}

-- Files grouped by crate for parallel processing.
-- Each entry: { path, crate, lines, description }
local FILE_GROUPS = {
    -- Group 1: luft-core contracts & types
    {
        crate = "luft-core",
        files = {
            { path = "crates/luft-core/src/contract/event.rs", lines = 280, desc = "Event types and EventSender" },
            { path = "crates/luft-core/src/contract/finding.rs", lines = 40, desc = "Finding type definition" },
            { path = "crates/luft-core/src/contract/mod.rs", lines = 24, desc = "Contract module re-exports" },
            { path = "crates/luft-core/src/mock_backend.rs", lines = 132, desc = "Mock backend for testing" },
            { path = "crates/luft-core/src/scheduler/config.rs", lines = 65, desc = "Scheduler configuration" },
            { path = "crates/luft-core/src/lib.rs", lines = 75, desc = "Library root re-exports" },
        }
    },
    -- Group 2: luft-runtime
    {
        crate = "luft-runtime",
        files = {
            { path = "crates/luft-runtime/src/sandbox.rs", lines = 407, desc = "Lua sandbox execution" },
            { path = "crates/luft-runtime/src/sdk/mod.rs", lines = 163, desc = "SDK module registration" },
            { path = "crates/luft-runtime/src/sdk/agent.rs", lines = 26, desc = "Agent module declaration" },
            { path = "crates/luft-runtime/src/sdk/agent/single.rs", lines = 100, desc = "Single agent execution" },
        }
    },
    -- Group 3: luft crate
    {
        crate = "luft",
        files = {
            { path = "crates/luft/src/builder.rs", lines = 437, desc = "Runtime builder pattern" },
            { path = "crates/luft/src/error.rs", lines = 39, desc = "Error types" },
            { path = "crates/luft/src/lib.rs", lines = 104, desc = "Library entry point" },
            { path = "crates/luft/src/prelude.rs", lines = 24, desc = "Prelude re-exports" },
        }
    },
    -- Group 4: luft-storage & luft-service
    {
        crate = "luft-storage+service",
        files = {
            { path = "crates/luft-storage/src/error.rs", lines = 27, desc = "Storage error types" },
            { path = "crates/luft-storage/src/lib.rs", lines = 42, desc = "Storage library root" },
            { path = "crates/luft-service/src/lib.rs", lines = 30, desc = "Service library root" },
        }
    },
    -- Group 5: luft-mcp
    {
        crate = "luft-mcp",
        files = {
            { path = "crates/luft-mcp/src/lib.rs", lines = 55, desc = "MCP library entry" },
        }
    },
    -- Group 6: luft-cli
    {
        crate = "luft-cli",
        files = {
            { path = "crates/luft-cli/src/config.rs", lines = 131, desc = "CLI configuration loading" },
            { path = "crates/luft-cli/src/commands/artifact_writer.rs", lines = 715, desc = "Artifact writer command" },
            { path = "crates/luft-cli/src/commands/backend.rs", lines = 357, desc = "Backend management command" },
            { path = "crates/luft-cli/src/commands/mock.rs", lines = 120, desc = "Mock data generation command" },
            { path = "crates/luft-cli/src/commands/save.rs", lines = 14, desc = "Save command" },
            { path = "crates/luft-cli/src/bin/fake_acp.rs", lines = 114, desc = "Fake ACP binary for testing" },
        }
    },
}

local RESULT_SCHEMA = {
    type = "object",
    properties = {
        files_tested = { type = "array", items = { type = "string" } },
        files_skipped = { type = "array", items = { type = "string" } },
        tests_written = { type = "integer" },
        summary = { type = "string" }
    },
    required = { "files_tested", "files_skipped", "tests_written", "summary" }
}

local VERIFY_SCHEMA = {
    type = "object",
    properties = {
        compile_passed = { type = "boolean" },
        tests_passed = { type = "boolean" },
        test_count = { type = "integer" },
        failures = { type = "array", items = { type = "string" } },
        fixes_applied = { type = "array", items = { type = "string" } },
        summary = { type = "string" }
    },
    required = { "compile_passed", "tests_passed", "test_count", "summary" }
}

function main()
    budget(600000, 30)

    phase("write_tests", #FILE_GROUPS)

    local results = parallel(FILE_GROUPS, function(group)
        local file_list = json.encode(group.files)
        return {
            prompt = "You are a senior Rust engineer adding unit tests to the Luft project "
                .. "(a Rust multi-agent orchestration runtime at the current working directory (.)).\n\n"
                .. "Your task: Add comprehensive #[cfg(test)] unit tests to EVERY file listed below in the '"
                .. group.crate .. "' module group.\n\n"
                .. "Files to add tests to:\n" .. file_list .. "\n\n"
                .. "## Instructions\n"
                .. "1. Read EACH file in full using your Read tool.\n"
                .. "2. For each file, identify ALL testable functions, methods, type conversions, "
                .. "error paths, edge cases, and serialization round-trips.\n"
                .. "3. Write idiomatic Rust unit tests covering:\n"
                .. "   - Happy path for every public function/method\n"
                .. "   - Error paths and edge cases (empty input, boundary values, invalid data)\n"
                .. "   - Serialization/deserialization round-trips where serde is involved\n"
                .. "   - Display/Debug formatting where applicable\n"
                .. "   - Default trait implementations\n"
                .. "   - From/Into conversions\n"
                .. "4. For files that are PURE re-export modules (lib.rs, mod.rs, prelude.rs):\n"
                .. "   - Add a basic test that verifies the re-exported items compile and are accessible\n"
                .. "   - Test that public API surface is complete (compile-time check)\n"
                .. "5. For files with complex logic (sandbox.rs, builder.rs, artifact_writer.rs):\n"
                .. "   - Test every branch and code path\n"
                .. "   - Use #[tokio::test] for async functions\n"
                .. "   - Test error conditions and boundary behavior\n"
                .. "6. Append tests to each file using the str_replace_based_edit_tool.\n"
                .. "   Place tests in a `#[cfg(test)] mod tests { use super::*; ... }` block at the end.\n"
                .. "   If the file already has a `#[cfg(test)]` block, ADD to it instead of creating a duplicate.\n"
                .. "7. Make sure ALL tests will COMPILE and PASS. Use proper imports.\n"
                .. "8. Do NOT modify any existing code — only append test blocks.\n\n"
                .. "## Important Notes\n"
                .. "- The project uses `luft_core`, `luft_runtime`, `luft_storage`, `luft_service`, "
                .. "`luft_adapters`, `luft_planner`, `luft_mcp`, `luft` (builder/mcp), `luft_cli` as crate names.\n"
                .. "- For testing types from other crates within the same workspace, use `use crate::...` "
                .. "or `use <crate_name>::...` as appropriate for the file's location.\n"
                .. "- Prefer focused unit tests over broad integration tests.\n"
                .. "- Aim for at least 3-5 test functions per non-trivial file.\n\n"
                .. "Return a JSON object matching the schema with the list of files you added tests to, "
                .. "files you skipped (and why), and the total number of test functions written.",
            schema = RESULT_SCHEMA,
            name = "write-tests-" .. group.crate,
            description = "Write unit tests for " .. group.crate .. " module group",
        }
    end)

    -- Collect results
    local all_tested = {}
    local all_skipped = {}
    local total_tests = 0
    for _, r in ipairs(results) do
        if r.ok and r.output then
            for _, f in ipairs(r.output.files_tested or {}) do
                table.insert(all_tested, f)
            end
            for _, f in ipairs(r.output.files_skipped or {}) do
                table.insert(all_skipped, f)
            end
            total_tests = total_tests + (r.output.tests_written or 0)
        end
    end
    log("Phase 1 complete: " .. #all_tested .. " files got tests, " .. total_tests .. " test functions written")

    phase("verify", 1)
    local verify_result = agent({
        prompt = "You are verifying the test coverage improvements in the Luft project at the current working directory (.).\n\n"
            .. "The following files just had unit tests added:\n" .. json.encode(all_tested) .. "\n\n"
            .. "Your task:\n"
            .. "1. Run `cargo test --workspace` using your shell tool.\n"
            .. "2. If there are compilation errors:\n"
            .. "   - Read the error messages carefully\n"
            .. "   - Fix each error by editing the test code (NOT the production code)\n"
            .. "   - Common fixes: wrong imports, missing trait bounds, incorrect type references\n"
            .. "   - Re-run cargo test after each fix round\n"
            .. "3. Repeat up to 5 fix rounds until all tests compile and pass.\n"
            .. "4. Count the total number of tests that pass.\n\n"
            .. "## Important\n"
            .. "- Do NOT delete tests to make compilation pass — FIX them instead.\n"
            .. "- If a test is fundamentally wrong (testing non-existent functionality), replace it with a correct test.\n"
            .. "- Make sure the doctests also pass (cargo test --doc).\n"
            .. "- Report any remaining failures with details.\n\n"
            .. "Return a JSON object matching the schema.",
        schema = VERIFY_SCHEMA,
        name = "verify-tests",
        description = "Verify all new tests compile and pass",
        timeout_ms = 300000,
    })

    phase("report", 1)
    local summary = "Coverage boost complete.\n"
        .. "Files with new tests: " .. #all_tested .. "\n"
        .. "Files skipped: " .. #all_skipped .. "\n"
        .. "Test functions written: " .. total_tests .. "\n"
    if verify_result.ok and verify_result.output then
        summary = summary
            .. "Compilation passed: " .. tostring(verify_result.output.compile_passed) .. "\n"
            .. "All tests passed: " .. tostring(verify_result.output.tests_passed) .. "\n"
            .. "Total tests: " .. (verify_result.output.test_count or 0) .. "\n"
            .. "Fixes applied: " .. #(verify_result.output.fixes_applied or {}) .. "\n"
            .. verify_result.output.summary
    end

    log(summary)
    report({
        workflow = "boost-coverage",
        files_with_new_tests = all_tested,
        files_skipped = all_skipped,
        test_functions_written = total_tests,
        verification = verify_result.output,
        summary = summary,
    })
end
