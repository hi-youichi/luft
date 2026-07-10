-- deep-research.lua — 多智能体「深度研究」工作流
--
-- 范式: 分解(decompose) → 并行调研(fan-out) → 综合(synthesize) → 校验润色(verify)。
-- 一个首席研究员把主题拆成若干子问题，每个子问题派一个子研究员并行调研，
-- 再由一名分析师综合成报告，最后由技术编辑做事实核查与收口，产出 Markdown。
--
-- 运行 (默认主题 = "Claude Code Dynamic Workflows"):
--   cargo run --bin luft -- run --workflow examples/deep-research.lua \
--       --backend opencode --approve -o claude-code-dynamic-workflows.md
--
-- 自定义主题 / 广度 (作为 JSON extra-args 传入):
--   cargo run --bin luft -- run --workflow examples/deep-research.lua \
--       --backend opencode --approve -o out.md '{"topic":"Rust async runtimes","breadth":5}'
--
-- 子研究员以「仓库当前目录」为 cwd 运行，因此可读取 ./docs 下的本地资料来夯实结论。

meta = {
  reasoning = "Multi-agent deep research workflow: decompose topic, parallel research, synthesize report, and verify facts",
  phases = {
    { label = "plan", dynamic = false },
    { label = "research", dynamic = false },
    { label = "synthesize", dynamic = false },
    { label = "verify", dynamic = false },
  },
}

----------------------------------------------------------------------
-- 参数
----------------------------------------------------------------------
local topic = (args and args.topic) or "Claude Code Dynamic Workflows"
local breadth = (args and tonumber(args.breadth)) or 4
if breadth < 2 then breadth = 2 end
if breadth > 8 then breadth = 8 end

----------------------------------------------------------------------
-- 工具函数
----------------------------------------------------------------------

-- 从 agent 结果里取出文本输出 (opencode 后端返回 {text=...})。
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

-- 清洗模型可能泄漏的前言/代码围栏：截到第一个一级标题，去掉整体 ``` 包裹。
local function clean_markdown(md)
    if type(md) ~= "string" then return md end
    md = md:gsub("^%s+", "")
    -- 去掉整体的 ```markdown ... ``` 包裹
    local fence_body = md:match("^```%w*\n(.*)\n```%s*$")
    if fence_body then md = fence_body end
    -- 如果正文前有前言文字，截到第一个 "# " 一级标题
    if md:sub(1, 2) ~= "# " then
        local idx = string.find(md, "\n# ", 1, true)
        if idx then md = string.sub(md, idx + 1) end
    end
    return md
end

-- 容错地调用 agent: 后端硬失败时降级为一个 error 结果，避免整条工作流中断。
local function safe_agent(opts)
    local ok, res = pcall(agent, opts)
    if ok and type(res) == "table" then return res end
    log("agent 调用失败，已降级: " .. tostring(res), "warn")
    return { status = "error", ok = false, output = { text = "" }, tokens = 0, findings = {} }
end

-- 从一段文本里抽出第一个 JSON 数组并解码 (失败返回 nil)。
local function extract_json_array(text)
    if type(text) ~= "string" then return nil end
    local s = string.find(text, "[", 1, true)
    if not s then return nil end
    local e, pos = nil, s
    while true do
        local nxt = string.find(text, "]", pos + 1, true)
        if not nxt then break end
        e, pos = nxt, nxt
    end
    if not e then return nil end
    local ok, decoded = pcall(json.decode, string.sub(text, s, e))
    if not ok then return nil end
    return decoded
end

-- 把解码结果规整成「问题字符串」列表。
local function normalize_questions(decoded, max_n)
    if type(decoded) ~= "table" then return nil end
    local out = {}
    for _, v in ipairs(decoded) do
        local q
        if type(v) == "string" then
            q = v
        elseif type(v) == "table" then
            q = v.question or v.q or v.title or v.text
        end
        if type(q) == "string" and #q > 0 then
            table.insert(out, q)
        end
    end
    if #out == 0 then return nil end
    while #out > max_n do table.remove(out) end
    return out
end

-- 分解失败时的兜底问题集 (保证工作流始终能产出结果)。
local function default_questions(t)
    return {
        "What is " .. t .. " — definition, purpose, and the core problem it solves?",
        "What is the architecture and execution model of " .. t
            .. " (how workflows are defined, compiled, and run)?",
        "What primitives and control flow does " .. t
            .. " provide (agents, parallelism, pipelines, sub-agents, tools, state)?",
        "What are the limitations, failure modes, and notable use cases or comparisons for " .. t .. "?",
    }
end

function main()
----------------------------------------------------------------------
-- 阶段 1: 分解 —— 首席研究员把主题拆成子问题
----------------------------------------------------------------------
phase("plan", 1)
log("研究主题: " .. topic)

local plan = safe_agent({
    prompt = table.concat({
        "You are the lead researcher planning a deep-research investigation.",
        'Topic: "' .. topic .. '".',
        "Break this topic into exactly " .. breadth .. " focused, non-overlapping research questions",
        "that together give comprehensive coverage (definition, architecture/mechanism,",
        "capabilities/primitives, limitations, comparisons, real-world use).",
        "You are running inside a code repository; you MAY read files under ./docs and",
        "consult https://code.claude.com/docs/en/workflows if you have web access.",
        "Output ONLY a JSON array of " .. breadth .. " question strings — nothing else.",
    }, "\n"),
})

local questions = normalize_questions(extract_json_array(out_text(plan)), breadth)
if not questions then
    log("分解结果无法解析，使用兜底问题集", "warn")
    questions = default_questions(topic)
end
while #questions > breadth do
    table.remove(questions)
end
log("生成 " .. #questions .. " 个子问题")

----------------------------------------------------------------------
-- 阶段 2: 并行调研 —— 每个子问题派一个子研究员
----------------------------------------------------------------------
phase("research", #questions)

local research = parallel(questions, function(q)
    return {
        prompt = table.concat({
            'You are a subject-matter researcher investigating ONE question for a report on "' .. topic .. '".',
            "Question: " .. q,
            "You are running inside the Luft code repository (current working directory).",
            "Relevant background notes may exist under ./docs (e.g. docs/research, docs/archive).",
            "Read them with your tools if present, and consult",
            "https://code.claude.com/docs/en/workflows and the web if you have access.",
            "Then answer the question thoroughly but concisely (~250-450 words).",
            "Ground every claim; prefer concrete mechanisms, primitives, and examples over generalities.",
            "End with a 'Sources:' line listing what you actually used (files/URLs), or 'Sources: model knowledge'.",
        }, "\n"),
    }
end)

----------------------------------------------------------------------
-- 阶段 3: 综合 —— 分析师把所有子调研合成结构化报告
----------------------------------------------------------------------
phase("synthesize", 1)

local material = { "# Research dossier on: " .. topic, "" }
for i, q in ipairs(questions) do
    table.insert(material, "## Sub-question " .. i .. ": " .. q)
    local r = research[i]
    table.insert(material, "Status: " .. ((r and r.status) or "missing"))
    table.insert(material, out_text(r))
    table.insert(material, "")
end
local dossier = table.concat(material, "\n")

local synth = safe_agent({
    prompt = table.concat({
        "You are a senior research analyst. Using ONLY the dossier below, write a comprehensive,",
        'well-structured Markdown report on "' .. topic .. '".',
        "Required sections:",
        "  1. # " .. topic .. "   (title, H1)",
        "  2. ## Executive Summary   (5-8 bullet points)",
        "  3. ## Background & Definition",
        "  4. ## Architecture & Execution Model",
        "  5. ## Core Primitives / Capabilities",
        "  6. ## Limitations & Open Questions",
        "  7. ## Key Takeaways",
        "Integrate and de-duplicate the sub-research; do not invent facts absent from the dossier.",
        "Output ONLY the Markdown report.",
        "",
        "----- DOSSIER -----",
        dossier,
    }, "\n"),
})

local draft_md = clean_markdown(out_text(synth))
if #draft_md == 0 then
    draft_md = dossier -- 综合失败兜底: 直接用 dossier
end

----------------------------------------------------------------------
-- 阶段 4: 校验润色 —— 技术编辑做事实核查与收口
----------------------------------------------------------------------
phase("verify", 1)

local verified = safe_agent({
    prompt = table.concat({
        "You are a meticulous technical editor and fact-checker.",
        'Below is a draft research report on "' .. topic .. '".',
        "Produce the FINAL report in clean Markdown:",
        "  - fix inaccuracies and remove redundancy,",
        "  - keep it well-structured and self-contained,",
        "  - append a final '## Confidence & Caveats' section listing any claims that are",
        "    uncertain, outdated, or that you could not verify.",
        "Output ONLY the final Markdown report (no preamble, no surrounding code fence).",
        "",
        "----- DRAFT -----",
        draft_md,
    }, "\n"),
})

local final_md = clean_markdown(out_text(verified))
if #final_md == 0 then final_md = draft_md end

----------------------------------------------------------------------
-- 汇总并产出报告
----------------------------------------------------------------------
local total_tokens = (plan.tokens or 0) + (synth.tokens or 0) + (verified.tokens or 0)
local ok_count = 0
for _, r in ipairs(research) do
    total_tokens = total_tokens + (r.tokens or 0)
    if r.status == "ok" then ok_count = ok_count + 1 end
end

log(string.format("研究完成: %d/%d 子问题成功, 约 %d tokens", ok_count, #questions, total_tokens))

report({
    topic = topic,
    questions = questions,
    sub_research_ok = ok_count,
    sub_research_total = #questions,
    total_tokens = total_tokens,
    -- `-o file.md` 会直接写出这段干净的 Markdown (见 cli 的报告落盘逻辑)。
    markdown = final_md,
})
end
