-- comprehensive-audit.lua — 全 crate 安全与质量审计（含对抗性验证）
--
-- 设计哲学: "脚本 = 编排器（复杂），Agent = 执行器（极简）"。
-- 编排层堆叠了动态发现、3 层嵌套 span、多阶段 pipeline、错误降级、
-- resume skip、对抗性多轮投票 —— 但每一个 agent() 调用只做一件小事:
--   列目录 / 列文件 / 找 unsafe 行 / 判严重度 / 打分 / 投 yes-no 票 / 写总结。
--
-- 运行:
--   cargo run --bin maestro -- run --workflow examples/comprehensive-audit.lua \
--       --backend opencode --approve -o audit-report.md
--   (mock: 将 --backend opencode 换成 --backend mock)
--
-- 注意（运行时真相，与 workflow-authoring-guide 存在差异）:
--   pipeline 的 stage handler 必须自行调用 agent() 并返回其 result；
--   返回值会成为下一 stage 的入参，runtime 不会替你执行 agent。
--   stage table 形式用 { label=, handler= }（非 guide 示例的 name=）。

--------------------------------------------
-- Goal:  Crate-wide security & quality audit with adversarial verification
-- Arch:
--   discover subsystems                          --> [subsystems[]]
--   for each subsystem:
--     +-- discover modules                       --> [modules[]]
--     +-- (pipeline) for each module:
--     |     +-- scan                             --> [SCAN]
--     |     +-- classify        (degrade on fail) --> [CLASS]
--     |     \-- score           (degrade on fail) --> [SCORE]
--   collect findings                             --> [findings[]]
--   repeat (<= N rounds):
--     +-- vote on findings (parallel)            --> [votes[]]
--     +-- keep survivors                         --> [survivors[]]
--     \-- break if converged
--   summarize survivors                          --> [REPORT]
-- Flow:  discover -> subsystems[] -> modules[] -> pipeline(scan,class,score) -> findings[] -> vote -> survivors -> report
--------------------------------------------

meta = {
  reasoning = "Comprehensive crate-wide security and quality audit with adversarial multi-round verification",
  phases = {
    { label = "discover subsystems", dynamic = false },
    { label = "audit subsystems", dynamic = true },
    { label = "adversarial verification", dynamic = false },
    { label = "summarize", dynamic = false },
  },
}

----------------------------------------------------------------------
-- Schemas（按字段访问 output 的 agent 调用都必须提供 schema）
----------------------------------------------------------------------

local SUBSYSTEMS_SCHEMA = {
    type = "object",
    properties = {
        subsystems = { type = "array", items = { type = "string" } }
    },
    required = { "subsystems" },
}

local MODULES_SCHEMA = {
    type = "object",
    properties = {
        files = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    file = { type = "string" },        -- 相对子系统目录的路径
                    description = { type = "string" },  -- 一句话说明
                },
                required = { "file" },
            },
        },
    },
    required = { "files" },
}

local SCAN_SCHEMA = {
    type = "object",
    properties = {
        issues = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    line = { type = "integer" },
                    kind = { type = "string" },   -- unsafe | panic | unwrap | todo | expect
                    snippet = { type = "string" },
                },
                required = { "line", "kind" },
            },
        },
    },
    required = { "issues" },
}

local CLASSIFY_SCHEMA = {
    type = "object",
    properties = {
        issues = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    line = { type = "integer" },
                    kind = { type = "string" },
                    severity = { type = "string" },  -- low | medium | high | critical
                    reason = { type = "string" },
                },
                required = { "line", "severity" },
            },
        },
    },
    required = { "issues" },
}

local SCORE_SCHEMA = {
    type = "object",
    properties = {
        score = { type = "integer" },               -- 0-100 健康分
        findings = {                                 -- 仅 high/critical 的条目
            type = "array",
            items = {
                type = "object",
                properties = {
                    line = { type = "integer" },
                    kind = { type = "string" },
                    severity = { type = "string" },
                    reason = { type = "string" },
                },
                required = { "line", "severity" },
            },
        },
    },
    required = { "score" },
}

local VOTE_SCHEMA = {
    type = "object",
    properties = {
        approve = { type = "boolean" },   -- true = 确认是真实风险
        reason = { type = "string" },
    },
    required = { "approve" },
}

local REPORT_SCHEMA = {
    type = "object",
    properties = {
        summary = { type = "string" },
        critical_count = { type = "integer" },
        top_risks = { type = "array", items = { type = "string" } },
    },
    required = { "summary" },
}

----------------------------------------------------------------------
-- 工具函数
----------------------------------------------------------------------

-- 用 pcall 包装 agent，避免单次失败击穿整个脚本。
local function safe_agent(opts)
    local ok, res = pcall(agent, opts)
    if ok and type(res) == "table" then return res end
    log("Agent 调用失败，已降级: " .. tostring(res), "warn")
    return { status = "error", ok = false, output = {}, tokens = 0, findings = {} }
end

function main()
----------------------------------------------------------------------
-- 阶段 1: 发现子系统（动态枚举，不硬编码）
----------------------------------------------------------------------
phase("discover subsystems", 1)
log("开始发现子系统...")

local discover = safe_agent({
    prompt = table.concat({
        "List the names of every top-level subdirectory under src/.",
        "Return ONLY subdirectory names (e.g. core, runtime, adapters).",
        "Do not include files like main.rs or lib.rs.",
    }, "\n"),
    schema = SUBSYSTEMS_SCHEMA,
})

if not discover.ok then
    report({ error = "子系统发现失败: " .. (discover.status or "unknown") })
    return
end

local subsystems = discover.output.subsystems or {}
if #subsystems == 0 then
    report({ error = "未发现任何子系统", subsystems = {} })
    return
end
log(string.format("发现 %d 个子系统: %s", #subsystems, table.concat(subsystems, ", ")))

----------------------------------------------------------------------
-- 阶段 2: 逐子系统审计（3 层嵌套 span + resume skip）
----------------------------------------------------------------------
local all_findings = {}
local file_scores = {}

for _, sys in ipairs(subsystems) do
    local gname = "audit " .. sys
    if completed_spans and completed_spans[gname] then
        log("跳过已完成: " .. gname)
        goto skip_sys
    end

    local g = phase_begin(gname)

    -- 2.1 发现该子系统下的模块文件
    phase("discover modules")
    local md = safe_agent({
        prompt = table.concat({
            "List every .rs file under src/" .. sys .. "/ (including nested subdirectories).",
            "For each file give a one-line description of its purpose.",
        }, "\n"),
        schema = MODULES_SCHEMA,
    })

    local modules = {}
    if md.ok and type(md.output) == "table" then
        modules = md.output.files or {}
    end
    log(string.format("子系统 %s: %d 个文件", sys, #modules))

    -- 2.2 为每个文件构造 pipeline item
    local pl_items = {}
    for _, f in ipairs(modules) do
        table.insert(pl_items, {
            file = "src/" .. sys .. "/" .. f.file,
            subsystem = sys,
        })
    end

    -- 2.3 多阶段流式管道: scan -> classify -> score
    --     每个 stage handler 自行调用 agent()，返回值流入下一 stage。
    --     失败时向下一 stage 喂最小默认值，而不是让管线崩溃。
    if #pl_items > 0 then
        local res = pipeline({
            items = pl_items,
            max_inflight = 4,
            stages = {
                {
                    label = "scan",
                    handler = function(item)
                        phase("scan " .. item.file)
                        local r = safe_agent({
                            prompt = table.concat({
                                "Read the file " .. item.file .. ".",
                                "List every line that contains one of:",
                                "  unsafe block, panic!/panic, .unwrap(), todo!(), .expect(",
                                "Return line number, the kind, and the code snippet.",
                                "If none found, return an empty issues array.",
                            }, "\n"),
                            schema = SCAN_SCHEMA,
                        })
                        if not r.ok then
                            return { file = item.file, ok = false, issues = {} }
                        end
                        return {
                            file = item.file,
                            ok = true,
                            issues = r.output.issues or {},
                        }
                    end,
                },
                {
                    label = "classify",
                    handler = function(prev)
                        if not prev.ok or #prev.issues == 0 then
                            return { file = prev.file, ok = false, classified = {} }
                        end
                        phase("classify " .. prev.file)
                        local r = safe_agent({
                            prompt = table.concat({
                                "For each issue below, assign a severity:",
                                "low | medium | high | critical.",
                                "Give a one-line reason. Keep the same line numbers.",
                                "",
                                json.encode(prev.issues),
                            }, "\n"),
                            schema = CLASSIFY_SCHEMA,
                        })
                        if not r.ok then
                            return { file = prev.file, ok = false, classified = {} }
                        end
                        return {
                            file = prev.file,
                            ok = true,
                            classified = r.output.issues or {},
                        }
                    end,
                },
                {
                    label = "score",
                    handler = function(prev)
                        if not prev.ok or #prev.classified == 0 then
                            return {
                                file = prev.file,
                                ok = true,
                                score = 100,
                                findings = {},
                            }
                        end
                        phase("score " .. prev.file)
                        local r = safe_agent({
                            prompt = table.concat({
                                "Given these classified issues, give the file a health",
                                "score from 0 (worst) to 100 (best). Then list ONLY the",
                                "high/critical findings in the findings array.",
                                "",
                                json.encode(prev.classified),
                            }, "\n"),
                            schema = SCORE_SCHEMA,
                        })
                        if not r.ok then
                            return {
                                file = prev.file,
                                ok = false,
                                score = 0,
                                findings = {},
                            }
                        end
                        -- 给每条 finding 打上文件来源，便于后续聚合与投票
                        local findings = r.output.findings or {}
                        for _, fnd in ipairs(findings) do
                            fnd.file = prev.file
                        end
                        return {
                            file = prev.file,
                            ok = true,
                            score = r.output.score or 0,
                            findings = findings,
                        }
                    end,
                },
            },
        })

        -- 2.4 聚合: pipeline_result.items[i].output 是最后一个 stage(score) 的返回值
        for _, it in ipairs(res.items or {}) do
            local out = it.output
            if type(out) == "table" then
                table.insert(file_scores, { file = out.file, score = out.score or 0 })
                if out.findings then
                    for _, fnd in ipairs(out.findings) do
                        table.insert(all_findings, fnd)
                    end
                end
            end
        end

        log(string.format("pipeline: %d ok / %d failed, %dms",
            res.ok or 0, res.failed or 0, res.total_elapsed_ms or 0))
    end

    phase_end(g)
    ::skip_sys::
end

log(string.format("共收集 %d 条待验证 findings", #all_findings))

----------------------------------------------------------------------
-- 阶段 3: 对抗性验证（parallel 投票 + 多轮收敛）
----------------------------------------------------------------------
phase("adversarial verification")

local survivors = all_findings
local max_rounds = 3

for round = 1, max_rounds do
    if #survivors == 0 then
        log("无 findings 需要验证，跳过对抗轮次")
        break
    end
    log(string.format("对抗轮次 %d: %d findings", round, #survivors))

    -- 每个 agent 只回答一个问题: 这条 finding 是不是真实风险？yes/no
    local votes = parallel(survivors, function(finding)
        return {
            prompt = table.concat({
                "A code audit produced this finding. Decide if it is a REAL risk",
                "that should be reported. Reply approve=true only if you are",
                "confident it is genuine and actionable.",
                "",
                json.encode(finding),
            }, "\n"),
            schema = VOTE_SCHEMA,
        }
    end)

    local kept = {}
    for i, finding in ipairs(survivors) do
        local v = votes[i]
        if v and v.ok and v.output.approve then
            table.insert(kept, finding)
        end
    end

    log(string.format("轮次 %d: %d / %d 通过", round, #kept, #survivors))

    -- 收敛: 没有被否决则提前结束
    if #kept == #survivors then
        log("已收敛，停止对抗验证")
        break
    end
    survivors = kept
end

local verified = survivors
log(string.format("验证后剩余 %d 条 findings", #verified))

----------------------------------------------------------------------
-- 阶段 4: 综合报告
----------------------------------------------------------------------
phase("summarize")

local summary = safe_agent({
    prompt = table.concat({
        "You are a senior Rust auditor. Write a concise audit summary IN CHINESE",
        "based on these verified findings and file scores.",
        "Return: summary (markdown text), critical_count, and top_risks (array",
        "of short strings, the 3 most important).",
        "",
        "Verified findings:",
        json.encode(verified),
        "",
        "File scores:",
        json.encode(file_scores),
    }, "\n"),
    schema = REPORT_SCHEMA,
})

local report_data = {
    subsystems_scanned = #subsystems,
    files_scored = #file_scores,
    findings_collected = #all_findings,
    findings_verified = #verified,
    rejection_rate = (#all_findings > 0)
        and string.format("%.0f%%", 100 * (1 - #verified / #all_findings))
        or "n/a",
    file_scores = file_scores,
    verified_findings = verified,
}

if summary.ok and type(summary.output) == "table" then
    report_data.summary = summary.output.summary or "(无摘要)"
    report_data.critical_count = summary.output.critical_count or 0
    report_data.top_risks = summary.output.top_risks or {}
else
    report_data.summary = "(摘要生成失败)"
end

report(report_data)
end
