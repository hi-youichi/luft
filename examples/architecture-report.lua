-- architecture-report.lua — 遍历所有模块并生成架构概览报告
--
-- 范式: 发现(discover) → 并行分析(fan-out) → 综合(synthesize) → 产出报告。
-- 工作流: 一个探测 agent 枚举所有模块，并行分析每个模块的源代码，
-- 最后合成一份中文架构概览 Markdown 报告。
--
-- 运行:
--   cargo run --bin luft -- run --workflow examples/architecture-report.lua \
--       --backend opencode --approve -o architecture-report.md

meta = {
  reasoning = "Discover and analyze all Rust modules in Luft codebase, then synthesize a comprehensive Chinese architecture overview report",
  phases = {
    { label = "discovery", dynamic = false },
    { label = "analysis", dynamic = true },
    { label = "synthesis", dynamic = false },
  },
}

----------------------------------------------------------------------
-- 工具函数
----------------------------------------------------------------------

local function out_text(res)
    if type(res) ~= "table" then return "" end
    local o = res.output
    if type(o) == "table" then
        if type(o.text) == "string" then return o.text end
        return json.encode(o)
    elseif type(o) == "string" then return o end
    return ""
end

local function safe_agent(opts)
    local ok, res = pcall(agent, opts)
    if ok and type(res) == "table" then return res end
    log("Agent 调用失败，已降级: " .. tostring(res), "warn")
    return { status = "error", ok = false, output = {}, tokens = 0, findings = {} }
end

function main()
----------------------------------------------------------------------
-- 阶段 1: 发现 —— 探测模块结构
----------------------------------------------------------------------
phase("discovery", 1)
log("开始探索模块结构...")

local discovery = safe_agent({
    prompt = table.concat({
        "You are exploring the Luft multi-agent orchestration runtime codebase",
        "(current working directory). Your task is to enumerate ALL Rust modules.",
        "",
        "Steps:",
        "  1. Read Cargo.toml for project metadata.",
        "  2. Read src/lib.rs to find top-level module declarations.",
        "  3. For EACH declared top-level module, explore its directory/files.",
        "  4. Read each .rs file's first ~30 lines to understand its purpose.",
        "  5. For sub-modules (e.g. core::contract, core::scheduler, runtime::sandbox),",
        "     recursively discover their files too.",
        "",
        "Return a JSON object with key 'modules' = array of:",
        "  {",
        "    name: string,         -- module name (e.g. 'core', 'cli')",
        "    path: string,         -- relative path under src/ (e.g. 'core', 'cli.rs')",
        "    submodules: [string], -- sub-module names if any",
        "    files: [{             -- .rs files in this module",
        "      file: string,       -- path relative to src/ (e.g. 'core/contract/backend.rs')",
        "      description: string -- one-line summary of the file's purpose",
        "    }]",
        "  }",
        "",
        "Include 'main.rs' and 'lib.rs' as separate entries.",
        "Be thorough — do NOT miss any module or .rs file.",
        "Read actual file contents to produce accurate descriptions.",
        "",
        "Output ONLY valid JSON — no surrounding text or code fences.",
    }, "\n"),
})

-- 将结果规整为模块列表，失败时用兜底结构。
local modules
if discovery.ok then
    local raw = discovery.output
    if type(raw) == "string" then
        local ok, decoded = pcall(json.decode, raw)
        if ok then raw = decoded end
    end
    if type(raw) == "table" and type(raw.modules) == "table" then
        modules = raw.modules
    end
end

if not modules or #modules == 0 then
    log("模块发现结果无法解析，使用兜底模块列表", "warn")
    modules = {
        { name = "core",      path = "core",      submodules = {"contract","scheduler","journal","mock_backend","state"}, files = {} },
        { name = "adapters",  path = "adapters",  submodules = {"acp_adapter","permission","result_collector","update_mapper"}, files = {} },
        { name = "runtime",   path = "runtime",   submodules = {"sandbox","converge","pipeline","error"}, files = {} },
        { name = "cli",       path = "cli.rs",    submodules = {}, files = {} },
        { name = "mcp",       path = "mcp.rs",    submodules = {}, files = {} },
        { name = "planner",   path = "planner.rs",submodules = {}, files = {} },
        { name = "main.rs",   path = "main.rs",   submodules = {}, files = {} },
        { name = "lib.rs",    path = "lib.rs",    submodules = {}, files = {} },
    }
end

log(string.format("发现 %d 个模块/文件，开始并行分析", #modules))

----------------------------------------------------------------------
-- 阶段 2: 并行分析 —— 每个模块独立分析
----------------------------------------------------------------------
phase("analysis", #modules)

local analyses = parallel(modules, function(m)
    return {
        prompt = table.concat({
            "You are a Rust code analyst. Analyze the Luft module '" .. m.name .. "'.",
            "Module location: src/" .. m.path,
            "Sub-modules: " .. json.encode(m.submodules or {}),
            "",
            "Read ALL source files in this module (including sub-module files).",
            "Then produce a JSON object with this exact structure:",
            "{",
            '  "module": "' .. m.name .. '",',
            '  "summary": "overall module purpose and responsibility",',
            '  "key_types": ["list of key structs, traits, enums defined"],',
            '  "responsibilities": ["list of key responsibilities"],',
            '  "dependencies": ["other Luft modules or external crates it depends on"],',
            '  "design_notes": "notable design decisions or patterns used",',
            '  "files": [{"file": "path", "summary": "what this file does, its key types, and its role"}]',
            "}",
            "",
            "For each file, cover:",
            "  1) Primary responsibility",
            "  2) Key types/structs/traits/enums it defines",
            "  3) Dependencies on other Luft modules",
            "  4) How it fits in the overall orchestration system",
            "",
            "Be specific — mention actual struct/enum/trait/function names.",
            "Output ONLY valid JSON — no other text.",
        }, "\n"),
    }
end)

----------------------------------------------------------------------
-- 阶段 3: 综合 —— 合并分析结果生成架构报告
----------------------------------------------------------------------
phase("synthesis", 1)
log("正在综合生成架构概览报告...")

local analysis_json = json.encode(analyses)

local report_result = safe_agent({
    prompt = table.concat({
        "You are a senior software architect. Synthesize the following module analyses",
        "into a comprehensive architecture overview report IN CHINESE for the Luft project",
        "(a multi-agent orchestration runtime written in Rust).",
        "",
        "Report structure (use Markdown):",
        "",
        "# Luft 架构概览报告",
        "",
        "## 1. 项目概述",
        "- What is Luft? Core purpose and problem it solves.",
        "- Technology stack summary.",
        "",
        "## 2. 整体架构",
        "- Layered/modular architecture description.",
        "- How modules relate — dependency direction and dependency graph.",
        "",
        "## 3. 模块职责说明",
        "- For EACH module:",
        "  - Module name",
        "  - Purpose and responsibility",
        "  - Key types, structs, traits (with actual names from the analysis)",
        "  - Dependencies on other modules",
        "  - Sub-modules breakdown (if any)",
        "  - Key files and their roles",
        "",
        "## 4. 模块间数据流",
        "- End-to-end workflow execution flow:",
        "  CLI command → planner → runtime → adapter → subagent → results back to user",
        "- Event flow, scheduling, state persistence.",
        "",
        "## 5. 关键设计决策",
        "- Why Lua as orchestration language?",
        "- Why ACP (Agent Communication Protocol)?",
        "- Sandbox design, pipeline/parallel/converge primitives.",
        "- Journal/checkpoint for resume.",
        "",
        "## 6. 依赖关系总览",
        "- Key external crates and what they provide.",
        "",
        "Be ACCURATE and SPECIFIC — reference actual type names, function names,",
        "and module paths from the analyses. Use ONLY the provided analyses as source.",
        "Do NOT add anything that is not supported by the analyses.",
        "",
        "Analyzed modules: " .. (function()
            local names = {}
            for _, m in ipairs(modules or {}) do
                table.insert(names, m.name)
            end
            return table.concat(names, ", ")
        end)() .. ".",
        "",
        "----- MODULE ANALYSES -----",
        analysis_json,
    }, "\n"),
})

local overview_md = out_text(report_result)
if #overview_md == 0 then
    overview_md = "# Luft 架构概览报告\n\n（报告生成失败，请检查 agent 日志。）"
end

----------------------------------------------------------------------
-- 汇总并产出
----------------------------------------------------------------------
local total_tokens = 0
local ok_count = 0
for _, r in ipairs(analyses) do
    total_tokens = total_tokens + (r.tokens or 0)
    if r.ok then ok_count = ok_count + 1 end
end
total_tokens = total_tokens + (discovery.tokens or 0) + (report_result.tokens or 0)

log(string.format("分析完成: %d/%d 模块分析成功, 约 %d tokens", ok_count, #modules, total_tokens))

report({
    title = "Luft 架构概览报告",
    modules_analyzed = #modules,
    module_names = (function()
        local names = {}
        for _, m in ipairs(modules) do
            table.insert(names, m.name)
        end
        return names
    end)(),
    successful_analyses = ok_count,
    total_tokens = total_tokens,
    markdown = overview_md,
})
end
