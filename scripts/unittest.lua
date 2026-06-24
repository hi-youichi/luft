meta = {
    phases = {
        { label = "coverage-scan", detail = "run llvm-cov to find uncovered lines per module", agents = 1, depends_on = {} },
        { label = "implement", detail = "write tests for uncovered modules in parallel batches", agents = 6, depends_on = { 1 } },
        { label = "verify", detail = "final test run and coverage check", agents = 1, depends_on = { 2 } }
    },
    reasoning = "Scan for all uncovered lines, then write tests in parallel batches of 6 modules sorted by lowest coverage first, then verify full test suite and final coverage"
}

--------------------------------------------------------------------------------
-- Schemas
--------------------------------------------------------------------------------

local COVERAGE_SCHEMA = {
    type = "object",
    properties = {
        modules = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    path = { type = "string" },
                    line_coverage_pct = { type = "number" },
                    uncovered_line_numbers = { type = "array", items = { type = "integer" } },
                    total_lines = { type = "integer" },
                    uncovered_count = { type = "integer" }
                },
                required = { "path", "line_coverage_pct", "uncovered_line_numbers" }
            }
        },
        overall_coverage = { type = "number" }
    },
    required = { "modules" }
}

local IMPLEMENT_SCHEMA = {
    type = "object",
    properties = {
        module = { type = "string" },
        tests_added = { type = "integer" },
        compile_passed = { type = "boolean" },
        tests_passed = { type = "boolean" },
        details = { type = "string" }
    },
    required = { "module", "tests_added", "compile_passed", "tests_passed" }
}

local VERIFY_SCHEMA = {
    type = "object",
    properties = {
        overall_coverage = { type = "number" },
        all_tests_pass = { type = "boolean" },
        modules_at_100 = { type = "integer" },
        modules_below_100 = { type = "integer" },
        remaining_modules = { type = "array", items = { type = "string" } },
        summary = { type = "string" }
    },
    required = { "overall_coverage", "all_tests_pass", "summary" }
}

--------------------------------------------------------------------------------
-- Helpers
--------------------------------------------------------------------------------

local function safe_agent(opts)
    local ok, res = pcall(agent, opts)
    if ok and type(res) == "table" then return res end
    log("agent call failed: " .. tostring(res), "warn")
    return { status = "error", ok = false, output = nil, tokens = 0, findings = {} }
end

local function truncate_lines(lines, max_n)
    if #lines <= max_n then return json.encode(lines) end
    local first = {}
    for i = 1, max_n do
        table.insert(first, lines[i])
    end
    return json.encode(first) .. " (and " .. (#lines - max_n) .. " more)"
end

--------------------------------------------------------------------------------
-- Main
--------------------------------------------------------------------------------

function main()
    phase("coverage-scan", 1)
    log("Running coverage scan to find all uncovered lines")

    local scan = safe_agent({
        prompt = "You are in the Maestro Rust project at /Users/apple/dev/maestro.\n"
            .. "Run `cargo llvm-cov --lib --json 2>&1` to get per-file line coverage data in JSON format.\n"
            .. "If `--json` is not supported, try `cargo llvm-cov --lib 2>&1` for the table summary,\n"
            .. "then `cargo llvm-cov report --lib --show-missing-lines 2>&1` or run\n"
            .. "`cargo llvm-cov --lib --lcov --output-path /tmp/maestro-cov.info 2>&1` and read that file.\n\n"
            .. "For each source file under src/ (exclude tests/, examples/, target/):\n"
            .. "  - path: relative path from workspace root, e.g. 'src/core/state.rs'\n"
            .. "  - line_coverage_pct: percentage of lines covered (0-100)\n"
            .. "  - uncovered_line_numbers: array of specific line numbers not covered by tests\n"
            .. "  - total_lines: total lines of code\n"
            .. "  - uncovered_count: number of uncovered lines\n\n"
            .. "Return ALL modules found, even those at 100%.\n"
            .. "Use as many commands as needed to get accurate line-level data.",
        schema = COVERAGE_SCHEMA,
        timeout_ms = 300000
    })

    if not scan.ok or not scan.output or not scan.output.modules then
        log("Coverage scan failed: " .. (scan.status or "no output"), "error")
        report({ error = "Coverage scan failed", status = scan.status })
        return
    end

    local modules = scan.output.modules
    table.sort(modules, function(a, b) return a.line_coverage_pct < b.line_coverage_pct end)

    local targets = {}
    for _, m in ipairs(modules) do
        if m.line_coverage_pct < 100 then
            table.insert(targets, m)
        end
    end

    log("Overall coverage: " .. scan.output.overall_coverage .. "%, "
        .. #targets .. " modules below 100%")

    if #targets == 0 then
        log("All modules already at 100% coverage!")
        report({
            overall_coverage = scan.output.overall_coverage,
            all_tests_pass = true,
            modules_at_100 = #modules,
            modules_below_100 = 0,
            remaining_modules = {},
            summary = "All modules already have 100% line coverage"
        })
        return
    end

    -- Log bottom 5 for visibility
    for i = 1, math.min(5, #targets) do
        log("  " .. targets[i].path .. ": " .. targets[i].line_coverage_pct
            .. "% (" .. targets[i].uncovered_count .. " lines uncovered)")
    end
    if #targets > 5 then
        log("  ... and " .. (#targets - 5) .. " more modules")
    end

    --------------------------------------------------------------------------
    -- Implementation: process targets in batches of 6
    --------------------------------------------------------------------------
    local batch_size = 6
    local start = 1
    local all_results = {}

    while start <= #targets do
        local end_idx = start + batch_size - 1
        if end_idx > #targets then end_idx = #targets end

        local batch = {}
        for i = start, end_idx do
            table.insert(batch, targets[i])
        end

        phase("implement", #batch)
        log("Batch: modules " .. start .. "-" .. end_idx .. " of " .. #targets)

        local results = parallel(batch, function(m)
            local lines_str = truncate_lines(m.uncovered_line_numbers, 30)

            return {
                prompt = "You are a Rust test specialist in the Maestro project at /Users/apple/dev/maestro.\n\n"
                    .. "MODULE: " .. m.path .. "\n"
                    .. "Current line coverage: " .. m.line_coverage_pct .. "%\n"
                    .. "Uncovered line numbers: " .. lines_str .. "\n"
                    .. "Uncovered count: " .. m.uncovered_count .. " out of " .. m.total_lines .. " total lines\n\n"
                    .. "TASK: Add unit tests to cover ALL uncovered lines in this module.\n\n"
                    .. "STEPS:\n"
                    .. "1. Read the full file: read /Users/apple/dev/maestro/" .. m.path .. "\n"
                    .. "2. Locate the existing #[cfg(test)] module (or mod tests { ... })\n"
                    .. "3. For EACH uncovered line, understand what function/expression it belongs to\n"
                    .. "4. Write one or more test functions that exercise exactly those uncovered paths:\n"
                    .. "   - For each function with uncovered branches, write tests for each branch\n"
                    .. "   - For error handling paths, write tests that trigger the error\n"
                    .. "   - For conditional logic, test all conditions\n"
                    .. "   - For functions returning Result, test both Ok and Err\n"
                    .. "5. Insert new test functions INSIDE the existing #[cfg(test)] module\n"
                    .. "6. After editing, run: cargo test --lib 2>&1\n"
                    .. "7. If compilation fails, read errors, fix tests, retry\n"
                    .. "8. If tests fail, read errors, fix tests, retry\n"
                    .. "9. Once all tests pass, report your results\n\n"
                    .. "CRITICAL RULES:\n"
                    .. "- NEVER modify any production code (outside #[cfg(test)])\n"
                    .. "- Follow existing test patterns (same style, imports, helper usage)\n"
                    .. "- Use #[tokio::test] for async tests, #[test] for sync\n"
                    .. "- Do NOT add any comments to the code\n"
                    .. "- Make sure every function inside #[cfg(test)] is a test function (has #[test] or #[tokio::test])\n"
                    .. "- Use the Editor tool to make precise edits to the file",
                schema = IMPLEMENT_SCHEMA,
                timeout_ms = 600000
            }
        end)

        local ok_count = 0
        for i, r in ipairs(results) do
            if r.ok and r.output then
                ok_count = ok_count + 1
                log("  " .. r.output.module .. ": +" .. r.output.tests_added
                    .. " tests (compile=" .. tostring(r.output.compile_passed)
                    .. ", pass=" .. tostring(r.output.tests_passed) .. ")")
            else
                local module_name = batch[i] and batch[i].path or "unknown"
                log("  FAILED: " .. module_name .. " - " .. (r.status or "no status"), "warn")
            end
        end
        log("Batch complete: " .. ok_count .. "/" .. #batch .. " succeeded")

        table.insert(all_results, { batch = start .. "-" .. end_idx, results = results })

        start = end_idx + 1
    end

    --------------------------------------------------------------------------
    -- Final verification
    --------------------------------------------------------------------------
    phase("verify", 1)
    log("Running final verification")

    local verify = safe_agent({
        prompt = "Run final verification for Maestro project at /Users/apple/dev/maestro.\n\n"
            .. "1. Run: cargo test --lib 2>&1\n"
            .. "   - Check ALL tests pass (0 failures)\n"
            .. "2. Run: cargo llvm-cov --lib --json 2>&1\n"
            .. "   - Or if JSON not available: cargo llvm-cov --lib 2>&1\n"
            .. "3. Parse the coverage output to determine:\n"
            .. "   - overall_coverage: overall line coverage percentage\n"
            .. "   - all_tests_pass: boolean\n"
            .. "   - modules_at_100: count of modules with exactly 100% coverage\n"
            .. "   - modules_below_100: count of modules still below 100%\n"
            .. "   - remaining_modules: list of paths of modules still below 100%\n"
            .. "   - summary: comprehensive text explaining results\n\n"
            .. "Return structured data with all fields.",
        schema = VERIFY_SCHEMA,
        timeout_ms = 300000
    })

    if not verify.ok or not verify.output then
        log("Verification failed: " .. (verify.status or "no output"), "error")
        local cov = scan.output.overall_coverage or 0
        report({
            overall_coverage = cov,
            all_tests_pass = false,
            modules_at_100 = 0,
            modules_below_100 = #targets,
            remaining_modules = {},
            summary = "Verification agent failed: " .. (verify.status or "no output")
        })
        return
    end

    report(verify.output)
end