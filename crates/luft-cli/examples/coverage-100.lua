-- coverage-100.lua — 全 crate 测试覆盖率提升到 100%
--
-- 设计哲学：一个文件一个 agent，pmap 并发执行。
-- 每个 coroutine 内跑两步 agent 链：
--   1. analyze → 读取文件，识别未覆盖的函数/分支/edge case
--   2. write   → 追加 #[test] 到文件末尾
--
-- 运行 (mock):
--   cargo run -- run --workflow examples/coverage-100.lua --backend mock
--
-- 运行 (真实 backend):
--   cargo run -- run --workflow examples/coverage-100.lua --backend opencode --approve
--
-- 自定义参数:
--   cargo run -- run --workflow examples/coverage-100.lua --backend opencode --approve \
--       '{"crates":["luft-core","luft-runtime"],"exclude":["main.rs","bin/"]}'

meta = {
  reasoning = "Boost test coverage to 100% across all crates: discover .rs files, then pmap analyze+write per file",
  phases = {
    { label = "discover", dynamic = false },
    { label = "write-tests", dynamic = false },
  },
}

----------------------------------------------------------------------
-- 参数
----------------------------------------------------------------------
local max_files = (args and tonumber(args.max_files)) or 0  -- 0 = unlimited
local exclude_patterns = (args and args.exclude) or { "main.rs", "bin/", "mock_gen.rs", "mock_file_backend.rs" }
local crate_filter = (args and args.crates) or {}  -- empty = all crates

----------------------------------------------------------------------
-- Schema 定义
----------------------------------------------------------------------

local ANALYZE_SCHEMA = {
    type = "object",
    properties = {
        items = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    target = { type = "string" },
                    gap_type = { type = "string" },
                    description = { type = "string" },
                    test_code = { type = "string" },
                },
                required = { "target", "test_code" },
            },
        },
    },
    required = { "items" },
}

----------------------------------------------------------------------
-- 工具函数
----------------------------------------------------------------------

-- 从 agent 结果中提取文本
local function out_text(res)
    if type(res) ~= "table" then return "" end
    local o = res.output
    if type(o) == "table" then
        if type(o.text) == "string" then return o.text end
        return json.encode(o)
    elseif type(o) == "string" then
        return o
    end
    return ""
end

-- 安全调用 agent
local function safe_agent(opts)
    local ok, res = pcall(agent, opts)
    if ok and type(res) == "table" then return res end
    log("agent 调用失败，已降级: " .. tostring(res), "warn")
    return { status = "error", ok = false, output = {}, tokens = 0, findings = {} }
end

-- 检查文件路径是否应被排除
local function should_exclude(path, patterns)
    for _, pat in ipairs(patterns) do
        if string.find(path, pat, 1, true) then
            return true
        end
    end
    return false
end

-- 检查 crate 是否在过滤列表中（空列表 = 全部）
local function crate_matches(crate_name, filter)
    if not filter or #filter == 0 then return true end
    for _, c in ipairs(filter) do
        if c == crate_name then return true end
    end
    return false
end

----------------------------------------------------------------------
-- 主函数
----------------------------------------------------------------------
function main()

----------------------------------------------------------------------
-- 阶段 1: 发现所有 .rs 源文件
----------------------------------------------------------------------
phase("discover", 1)
log("开始发现 .rs 源文件...")

local discover = safe_agent({
    prompt = table.concat({
        "List all .rs source files under crates/ directory (recursively).",
        "For each file, provide its relative path from the repo root (e.g. crates/luft-core/src/lib.rs).",
        "Exclude any file that contains 'main.rs', 'bin/', 'mock_gen.rs', or 'mock_file_backend.rs' in its path.",
        "Also exclude files under tests/ subdirectory.",
        "Return the complete list.",
    }, "\n"),
    schema = {
        type = "object",
        properties = {
            files = {
                type = "array",
                items = {
                    type = "object",
                    properties = {
                        path = { type = "string" },
                        crate_name = { type = "string" },
                    },
                    required = { "path" },
                },
            },
        },
        required = { "files" },
    },
})

local all_files = {}
if discover.ok and type(discover.output) == "table" then
    local raw = discover.output.files or {}
    for _, f in ipairs(raw) do
        local path = f.path or ""
        local crate = f.crate_name or ""
        -- 应用过滤
        if not should_exclude(path, exclude_patterns)
            and crate_matches(crate, crate_filter) then
            table.insert(all_files, { path = path, crate = crate })
        end
    end
end

-- 兜底：如果 agent 发现失败，用硬编码列表
if #all_files == 0 then
    log("agent 发现失败，使用硬编码文件列表", "warn")
    local hardcoded = {
        "crates/luft-core/src/lib.rs",
        "crates/luft-core/src/journal.rs",
        "crates/luft-core/src/run_dir.rs",
        "crates/luft-core/src/state.rs",
        "crates/luft-core/src/mock_backend.rs",
        "crates/luft-core/src/contract/backend.rs",
        "crates/luft-core/src/contract/cache.rs",
        "crates/luft-core/src/contract/event.rs",
        "crates/luft-core/src/contract/finding.rs",
        "crates/luft-core/src/contract/ids.rs",
        "crates/luft-core/src/contract/mod.rs",
        "crates/luft-core/src/contract/schema.rs",
        "crates/luft-core/src/scheduler/config.rs",
        "crates/luft-core/src/scheduler/error.rs",
        "crates/luft-core/src/scheduler/mod.rs",
        "crates/luft-core/src/scheduler/registry.rs",
        "crates/luft-runtime/src/lib.rs",
        "crates/luft-runtime/src/error.rs",
        "crates/luft-runtime/src/pipeline.rs",
        "crates/luft-runtime/src/converge.rs",
        "crates/luft-runtime/src/sandbox.rs",
        "crates/luft-runtime/src/sdk/mod.rs",
        "crates/luft-runtime/src/sdk/agent.rs",
        "crates/luft-runtime/src/sdk/agent/journal.rs",
        "crates/luft-runtime/src/sdk/agent/parallel.rs",
        "crates/luft-runtime/src/sdk/agent/pmap.rs",
        "crates/luft-runtime/src/sdk/agent/single.rs",
        "crates/luft-runtime/src/sdk/control.rs",
        "crates/luft-runtime/src/sdk/convert.rs",
        "crates/luft-runtime/src/sdk/report.rs",
        "crates/luft-runtime/src/sdk/task.rs",
        "crates/luft-runtime/src/sdk/workflow.rs",
        "crates/luft-storage/src/lib.rs",
        "crates/luft-storage/src/db.rs",
        "crates/luft-storage/src/error.rs",
        "crates/luft-storage/src/reader.rs",
        "crates/luft-storage/src/writer.rs",
        "crates/luft-service/src/lib.rs",
        "crates/luft-service/src/phases.rs",
        "crates/luft-service/src/query.rs",
        "crates/luft-service/src/run.rs",
        "crates/luft-adapters/src/lib.rs",
        "crates/luft-adapters/src/acp_adapter.rs",
        "crates/luft-adapters/src/permission.rs",
        "crates/luft-adapters/src/result_collector.rs",
        "crates/luft-adapters/src/update_mapper.rs",
        "crates/luft-planner/src/lib.rs",
        "crates/luft-planner/src/meta.rs",
        "crates/luft-planner/src/types.rs",
        "crates/luft-mcp/src/lib.rs",
        "crates/luft-mcp/src/protocol.rs",
        "crates/luft-mcp/src/resources.rs",
        "crates/luft-mcp/src/server.rs",
        "crates/luft-mcp/src/tools.rs",
        "crates/luft/src/lib.rs",
        "crates/luft/src/builder.rs",
        "crates/luft/src/error.rs",
        "crates/luft/src/mcp.rs",
        "crates/luft/src/prelude.rs",
        "crates/luft-cli/src/config.rs",
        "crates/luft-cli/src/logging.rs",
        "crates/luft-cli/src/backend.rs",
        "crates/luft-cli/src/signal.rs",
        "crates/luft-cli/src/commands/mod.rs",
        "crates/luft-cli/src/commands/artifact_writer.rs",
        "crates/luft-cli/src/commands/backend.rs",
        "crates/luft-cli/src/commands/clear.rs",
        "crates/luft-cli/src/commands/event_log.rs",
        "crates/luft-cli/src/commands/generate.rs",
        "crates/luft-cli/src/commands/list.rs",
        "crates/luft-cli/src/commands/logs.rs",
        "crates/luft-cli/src/commands/lua_validate.rs",
        "crates/luft-cli/src/commands/mcp_server.rs",
        "crates/luft-cli/src/commands/mock.rs",
        "crates/luft-cli/src/commands/phase_renderer.rs",
        "crates/luft-cli/src/commands/phases.rs",
        "crates/luft-cli/src/commands/run.rs",
        "crates/luft-cli/src/commands/save.rs",
        "crates/luft-cli/src/commands/status.rs",
        "crates/luft-cli/src/commands/workflows.rs",
    }
    for _, p in ipairs(hardcoded) do
        if not should_exclude(p, exclude_patterns) then
            table.insert(all_files, { path = p, crate = "" })
        end
    end
end

-- 应用 max_files 限制
if max_files > 0 and #all_files > max_files then
    while #all_files > max_files do
        table.remove(all_files)
    end
end

log(string.format("共发现 %d 个 .rs 文件需要提升覆盖率", #all_files))

----------------------------------------------------------------------
-- 阶段 2: pmap 并发 — 每个文件一个 coroutine (analyze → write)
----------------------------------------------------------------------
phase("write-tests", #all_files)

local results = pmap(all_files, function(file_info)
    local path = file_info.path

    ------------------------------------------------------------------
    -- Step 1: 分析文件，找出未覆盖的函数/分支
    ------------------------------------------------------------------
    local analysis = safe_agent({
        prompt = table.concat({
            "You are a Rust test coverage expert.",
            "Read the file " .. path .. " completely.",
            "Identify ALL functions, methods, enum variants, and code paths",
            "that are NOT covered by existing #[test] or #[cfg(test)] mod tests.",
            "",
            "For each uncovered item, generate a complete #[test] function",
            "that exercises it. The test must:",
            "  1. Be syntactically valid Rust that compiles",
            "  2. Use proper assertions (assert_eq!, assert!, etc.)",
            "  3. Cover normal inputs, edge cases, and error paths",
            "  4. Use `super::*` import to access the module under test",
            "  5. Not require external dependencies beyond what the crate already uses",
            "  6. Not modify production code — only add test code",
            "",
            "If the file already has 100% coverage, return an empty items array.",
            "",
            "Return each test as a complete string (test_code field).",
        }, "\n"),
        schema = ANALYZE_SCHEMA,
    })

    if not analysis.ok then
        return { file = path, ok = false, error = "analysis failed", tests_written = 0 }
    end

    local items = {}
    if type(analysis.output) == "table" and analysis.output.items then
        items = analysis.output.items or {}
    end

    if #items == 0 then
        return { file = path, ok = true, tests_written = 0, status = "already_covered" }
    end

    ------------------------------------------------------------------
    -- Step 2: 将生成的测试追加到文件末尾的 #[cfg(test)] mod tests
    ------------------------------------------------------------------
    local test_code_parts = {}
    for _, item in ipairs(items) do
        if item.test_code and #item.test_code > 0 then
            table.insert(test_code_parts, item.test_code)
        end
    end

    if #test_code_parts == 0 then
        return { file = path, ok = true, tests_written = 0, status = "no_code_generated" }
    end

    local write_result = safe_agent({
        prompt = table.concat({
            "You are a Rust developer. Append the following test functions",
            "to the existing #[cfg(test)] mod tests block in " .. path .. ".",
            "If the file does not have a #[cfg(test)] mod tests block,",
            "create one at the end of the file.",
            "",
            "Rules:",
            "  1. Do NOT modify any existing code (production or test)",
            "  2. Only APPEND new #[test] functions inside the tests module",
            "  3. Use `use super::*;` for imports if not already present",
            "  4. Each test function must be self-contained",
            "",
            "Tests to add:",
            table.concat(test_code_parts, "\n\n    "),
        }, "\n"),
    })

    if not write_result then
        return { file = path, ok = false, error = "write failed", tests_written = 0 }
    end

    if write_result.ok then
        return {
            file = path,
            ok = true,
            tests_written = #test_code_parts,
            gaps_addressed = #items,
        }
    else
        return {
            file = path,
            ok = false,
            error = "write agent failed",
            tests_written = 0,
        }
    end
end)

----------------------------------------------------------------------
-- 汇总报告
----------------------------------------------------------------------
local total_ok = 0
local total_failed = 0
local total_tests_written = 0
local total_gaps = 0
local already_covered = 0

for _, r in ipairs(results) do
    if r.ok then
        total_ok = total_ok + 1
        total_tests_written = total_tests_written + (r.tests_written or 0)
        total_gaps = total_gaps + (r.gaps_addressed or 0)
        if r.status == "already_covered" then
            already_covered = already_covered + 1
        end
    else
        total_failed = total_failed + 1
    end
end

log(string.format("覆盖率提升完成: %d/%d 文件成功, %d 个测试已写入, %d 文件已全覆盖",
    total_ok, #all_files, total_tests_written, already_covered))

report({
    files_total = #all_files,
    files_ok = total_ok,
    files_failed = total_failed,
    tests_written = total_tests_written,
    gaps_addressed = total_gaps,
    already_covered = already_covered,
    results = results,
})
end
