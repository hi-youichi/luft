# WS 协议测试工具设计

> 为 `maestro serve` 设计一个 CLI 命令 `maestro test-ws`，在 Web UI 开发之前按设计场景逐条验证 WebSocket 协议实现，确保所有消息收发行为符合 [websocket-server.md](websocket-server.md) 的约定。
>
> **核心思想**：一份 code review 工作流脚本执行时覆盖 90% 的 `AgentEvent` 类型 + 9/10 的 Lua SDK 方法，WS 客户端在事件流上做断言。

---

## 1. 背景与目标

[`maestro serve`](websocket-server.md) 暴露 11 种 `ClientMsg`、13 种 `ServerMsg`、12 种 `AgentEvent`，是 [Web UI](web-ui.md) 的唯一天空。在 UI 投入开发前，需要确保 WS 协议层的行为可被可靠验证。

目标：

- 一份 **80 行的 code review Lua 脚本** 覆盖 `agent()` / `parallel()` / `converge()` / `pipeline()` / `phase()` / `log()` / `report()` / `json.decode()` / `budget()` —— 9/10 SDK 方法
- 可作为 CI 集成测试框架复用（与 [websocket-server.md §10 P7](websocket-server.md#10-实现计划) 对齐）

## 4. Code Review 工作流脚本

### 4.1 `code_review.lua` — 主工作流（覆盖 9/10 SDK 方法）

```lua
-- code_review.lua
-- 覆盖: agent / parallel / converge / pipeline / phase / log / report / json.decode / budget
-- 预期产出: ~40 条 AgentEvent，含全部 12 种类型（不含 acp_raw、cancelled）

-- 预算：60s 上限（MockBackend 极快，不会触发 —— timed_out 由 R5 单测）
budget({ time_ms = 60000 })

-- P0: 并行审查 3 个关键文件
phase("P0: Recon", 3)
local targets = { "src/auth.lua", "src/db.lua", "src/api.lua" }

local findings = parallel(targets, function(file)
    local code = "function " .. file:gsub("%.lua$", "_impl") .. "() return 42 end"

    local r = agent({
        prompt = string.format(
            [[审查文件 %s 的安全漏洞：
重点关注：SQL 注入、XSS、缺失鉴权、硬编码密钥。

代码：
```lua
%s
```

以 JSON 格式返回审查结果：
{
  "file": "%s",
  "issues": [
    { "severity": "critical|high|medium|low", "kind": "...", "title": "...", "location": "..." }
  ]
}]],
            file, code, file
        ),
        model = "claude",
        schema = {
            type = "object",
            properties = {
                file = { type = "string" },
                issues = {
                    type = "array",
                    items = {
                        type = "object",
                        properties = {
                            severity = { type = "string" },
                            kind = { type = "string" },
                            title = { type = "string" },
                            location = { type = "string" },
                        },
                    },
                },
            },
        },
        temperature = 0.2,
    })

    -- 验证 agent 返回的是有效 JSON
    local parsed = json.decode(r.text)
    log(string.format("✓ %s: 发现 %d 个问题", file, #(parsed.issues or {})), "info")
    return parsed
end)

-- P1: 交叉验证 findings
phase("P1: Cross-check", 2)

local verified = converge(findings, {
    mode = "parallel",
    count = 2,
    refine_count = 1,
    model = "claude",

    map = function(finding)
        return agent({
            prompt = string.format(
                [[验证以下安全审查发现是否属实，给出 confirmed / false_positive / uncertain：

原始发现：
%s

请返回：{ "verdict": "confirmed|false_positive|uncertain", "note": "..." }]],
                json.encode(finding)
            ),
            model = "claude",
            temperature = 0.1,
        })
    end,

    reduce = function(results)
        local confirmed = 0
        local fps = 0
        for _, r in ipairs(results) do
            if r.verdict == "confirmed" then
                confirmed = confirmed + 1
            elseif r.verdict == "false_positive" then
                fps = fps + 1
            end
        end
        return { confirmed = confirmed, false_positives = fps, total = #results }
    end,
})

log(string.format("交叉验证完成: %d/%d 确认为真实问题", verified.confirmed, verified.total), "info")

-- Pipeline: 生成最终报告
local report_data = pipeline(
    { findings = findings, verified = verified },
    {
        audit = function(items)
            local r = agent({
                prompt = string.format(
                    [[根据以下审查结果，生成一份结构化安全审计报告（Markdown 格式）：

审查结果：%s

验证结果：%d 个问题中 %d 个确认为真实问题

报告需包含：摘要、按 severity 分组的问题列表、修复建议]],
                    json.encode(items.findings),
                    items.verified.total,
                    items.verified.confirmed
                ),
                model = "claude",
            })
            return r.text
        end,
    }
)

-- 输出最终报告
report({ markdown = report_data.audit })

log("审查完成", "info")
```

### 4.2 `budget_timeout.lua` — R5 专用（覆盖 timed_out 路径）

```lua
-- budget_timeout.lua
-- 覆盖: budget() 触发 timed_out 路径
-- 预期: 每个 agent 以 status=timed_out 结束

budget({ time_ms = 1 })  -- 1ms 内未完成则超时

local r = agent({
    prompt = "审查 src/auth.lua 的鉴权逻辑",
})

-- 预期 agent_done { status: "timed_out" }
local s = r.status  -- 应为 "timed_out"
report({ markdown = "# Budget Test\nAgent status: " .. tostring(s) })
```

---


### 6.3 SDK 方法 × 工作流脚本

| SDK 方法 | 出现位置 |
|---------|---------|
| `agent()` | parallel(×3) + converge(×2) + pipeline(×1) + budget_timeout(×1) |
| `parallel()` | P0 · 3 个文件并行审查 |
| `converge()` | P1 · 2 个验证者 |
| `pipeline()` | 生成审计报告 |
| `phase()` | P0 + P1 |
| `log()` | 审查进度 + 交叉验证结果 |
| `report()` | 输出 Markdown 报告 |
| `json.encode()` | converge 的 prompt 中 |
| `json.decode()` | agent 返回解析 |
| `budget()` | code_review.lua 顶行（60s）+ budget_timeout.lua（1ms） |

`workflow()` 是唯一未覆盖的 SDK 方法——它启动子工作流并产生独立的 agent 事件，若需覆盖可在 Cargo 工作目录放一份 `sub/report_gen.lua` 脚本，code_review.lua 末尾加一行 `workflow("sub/report_gen", { data = report_data })`。子工作流走同一条事件总线，无需额外 WS 步骤。

---

## 相关文档

- WebSocket 服务器：[websocket-server.md](websocket-server.md)（消息协议、错误码、实现计划）
- Web UI 设计：[web-ui.md](web-ui.md)（实时事件消费方）
- Lua SDK 参考：[../sdk-reference.md](../sdk-reference.md)（SDK 方法签名与用法）
- 架构总览：[../architecture.md](../architecture.md)
