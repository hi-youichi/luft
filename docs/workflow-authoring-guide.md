# Maestro Lua Workflow 编写指南

> **定位**: 面向 workflow 编写者的实践指南，源自 planner 系统 prompt（`src/planner.rs` `LUA_DSL_REFERENCE`）。
>
> **蓝本**: `src/planner.rs:338-852` — 发给 planner LLM 的 DSL 规范，包含了编排理念、原语语义、设计方法论和完整示例。本文将该 prompt 的内容重组为人类可读的指南。
>
> **互补文档**:
> - [`lua-workflow-spec.md`](./dev/lua-workflow-spec.md) — 技术规范（API 精确定义、文件格式、验证规则）
> - [`sdk-reference.md`](./sdk-reference.md) — SDK 原语快速参考
> - [`architecture/planner.md`](./architecture/planner.md) — planner 模块架构

---

## 目录

1. [范式与执行模型](#1-范式与执行模型)
2. [工作流架构注释](#2-工作流架构注释)
3. [任务分解方法论](#3-任务分解方法论)
4. [原语使用指南](#4-原语使用指南)
5. [全局变量与环境](#5-全局变量与环境)
6. [Resume 模式](#6-resume-模式)
7. [错误处理与降级](#7-错误处理与降级)
8. [对抗性验证模式](#8-对抗性验证模式)
9. [编写规则速查](#9-编写规则速查)
10. [完整示例](#10-完整示例)

---

## 1. 范式与执行模型

### 1.1 NL → Lua 编译

Maestro 采用 **Claude Code Dynamic Workflows** 范式：用户给出自然语言任务，planner LLM 充当"编译器"，将 NL 编译为一段 Lua 编排脚本，runtime 再确定性执行该脚本。

```
  用户 NL 任务 ──► planner LLM ──► Lua 脚本 ──► 沙箱执行
       │              ▲                           │
       │              │                           ▼
       └──── LUA_DSL_REFERENCE               agent() 调用 ──► 调度器
       (本指南的蓝本)                             │
                                                 ▼
                                               report()
```

### 1.2 脚本 = 编排器，Agent = 执行器

这是 Maestro 最核心的设计原则：

| 层 | 职责 | 能做什么 | 不能做什么 |
|----|------|---------|-----------|
| **Lua 脚本** | 编排逻辑 | 持有循环、分支、中间结果；调用 `agent()` 分配任务 | 读文件、执行命令、访问网络 |
| **Agent** | 实际工作 | 读文件、grep、编辑、web 搜索、代码分析 | — |

脚本运行在 **沙箱** 中：`io`、`os`、`require`、文件和 shell 访问全部禁用。所有真实工作——读文件、搜索、编辑、分析——都由脚本通过 `agent()` 生成的子 Agent 完成。工作指令写在 Agent 的 prompt 文本中；Agent 有工具，脚本没有。

> **直觉**：把脚本想象成一个"不碰键盘的项目经理"——它只负责分配任务、收集结果、做决策，具体执行全部委托给 Agent。

### 1.3 Meta 声明与 main() 入口

每个脚本 **必须** 声明 `meta` 表和 `function main()` 入口。运行时在执行 `main()` 之前先提取 `meta`，向 CLI 发送 `PlanPreview` 事件，渲染出 phase 预览。

```lua
meta = {
  reasoning = "先发现子系统，逐个审计，对抗验证后汇总",
  phases = {
    {
      label = "discover subsystems",
      description = "Enumerate modules and group by subsystem",
      agents = 1,
      depends_on = {}
    },
    {
      label = "audit subsystems",
      description = "Per-subsystem review running in parallel",
      agents = 8,
      depends_on = { 0 },
      dynamic = true  -- 子项运行时动态发现
    },
    {
      label = "adversarial verification",
      description = "Cross-check critical findings against alternative agents",
      agents = 4,
      depends_on = { 1 }
    },
    {
      label = "summarize",
      description = "Aggregate all reviews + verified findings into final report",
      agents = 1,
      depends_on = { 2 }
    },
  },
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `reasoning` | string | ✓ | 一句话描述工作流策略（英文） |
| `phases` | table | ✓ | 顶层结构性 phase 列表，**不需要**列出每个运行时 `phase()` 调用 |
| `phases[].label` | string | ✓ | phase 名称（用于 CLI 渲染） |
| `phases[].description` | string |  | 一行说明（CLI 渲染用；推荐填，否则只显示 label） |
| `phases[].agents` | int |  | 计划 agent 数（`parallel` 时为 fan-out 数；用于 progress display） |
| `phases[].depends_on` | int[] |  | 依赖的 phase 索引列表（如 `{0}` 表示 phase 0 完成后才执行；空数组 `{}` 或省略 = 无依赖） |
| `phases[].dynamic` | bool |  | `true` = 该 phase 的子项在运行时动态发现（如 `for each` 循环内） |

**执行时序**：

```
① lua.load(script).exec()      顶层赋值：meta={...}, local schema, function main()
② extract_meta()               读取 meta.phases, meta.reasoning
③ events.send(PlanPreview)     CLI 渲染 "📋 Plan" 预览块
④ main().call()                正式执行编排逻辑 → phase/agent 事件流
```

**脚本骨架**：

```lua
--------------------------------------------
-- Goal:  ...
-- Arch:  ...
-- Flow:  ...
--------------------------------------------
meta = {
  reasoning = "...",
  phases = { { label = "..." }, ... },
}

-- schema 表和 helper 函数在顶层声明（main 可作为 upvalue 访问）
local SCHEMA = { ... }
local function helper() ... end

-- 所有执行代码包进 main()
function main()
  phase("discover")
  local r = agent({ prompt = "...", schema = SCHEMA })
  report({ result = r.output })
end
```

> **为什么需要 main()？** 没有 main() 时 `exec()` 会一口气执行所有 agent/phase 调用，无法在执行前插入预览。有了 main()，顶层 exec 只执行 meta 赋值 + 函数定义（安全无副作用），提取 meta 后再调 `main()` 正式执行。

---

## 2. 工作流架构注释

### 2.1 Plan-then-Code

每个脚本 **必须** 以一个结构化的头部注释开头。这强制编写者先规划再编码，使生成的脚本可读、可审计。

### 2.2 格式

```lua
--------------------------------------------
-- Goal:  <一行英文，描述工作流产出什么>
-- Arch:
--   <ASCII 树，展示控制流层级和产出物>
-- Flow:  <一行数据流链>
--------------------------------------------
```

三行分隔符各为 **44 个短横线**。

### 2.3 字段说明

| 字段 | 内容 | 示例 |
|------|------|------|
| `Goal` | 一行英文，说明工作流产出什么 | `Refactor auth, db, api modules` |
| `Arch` | ASCII 树，展示控制流层级 | 见下方标记法 |
| `Flow` | 全局数据流链，用 `->` 连接产出物 | `{modules} -> ANALYSIS -> CHANGES -> VERIFY -> report` |

### 2.4 Arch 标记法

```
A -> B            同一节点内的顺序步骤
for each X:       遍历发现的或静态的列表
repeat (<= N):    有界重试/迭代循环
break if X        提前退出条件
(parallel)        parallel() 扇出的内联标记
(pipeline)        pipeline() 阶段的内联标记
+-- node          子分支（后面还有兄弟节点）
\-- node          最后一个子分支（没有更多兄弟了）
--> [artifact]    该步骤产出的产出物（后缀）
(degrade on fail) 错误降级策略的内联标记
```

**缩进规则**：子分支比父节点多缩进 2 个空格。

### 2.5 完整示例

```
--------------------------------------------
-- Goal:  Refactor entire crate by subsystem
-- Arch:
--   discover subsystems                     --> [subsystems[]]
--   for each subsystem:
--     +-- discover modules                  --> [modules[]]
--     +-- for each module:
--           +-- analyze                     --> [ANALYSIS]
--           +-- change                      --> [CHANGES]
--           \-- verify                      --> [VERIFY]
-- Flow:  discover -> subsystems[] -> modules[] -> changes -> report
--------------------------------------------
```

**规则**：保持简洁诚实，省略不适用的细节。如果任务做了分解（见 [§3](#3-任务分解方法论)），Arch 树 **必须** 通过 `for each X:` 分支展示分解维度。

---

## 3. 任务分解方法论

### 3.1 何时分解

将大任务拆分为更小、独立的工作单元。每个单元成为一个 phase span；span 内部运行相似的微型工作流（如 `analyze → change → verify`）。

**需要分解的场景**：

- 任务涉及多个文件、模块、子系统或文档
- 任务有多个可独立描述的阶段（如"发现问题 → 修复 → 验证"）
- 范围未知或较大——先派 Agent 枚举目标，再逐个循环

**不需要分解的场景**：

- 单文件、单步骤任务——线性脚本即可

### 3.2 粒度控制

| 层级 | 粒度 | 典型内部结构 |
|------|------|-------------|
| **一个 span** | 一个工作单元（一个模块/文件/子系统/文档） | 2–4 个 phase 的固定微型工作流 |
| **span 内部** | `analyze → change → verify`（示例） | 每个 phase 一个 `agent()` 调用 |

**原则**：
- 每个 span 内复用相同的 phase 序列，使工作流统一、可预测
- **不要** 把所有工作塞进一个巨大 `agent()` 调用——无法验证、无法恢复、prompt 过长
- **不要** 过度拆分成只有单个 agent、无内部 phase 的 span——那不是工作单元，只是一个步骤，用 `phase()` 即可

### 3.3 分解维度

根据任务性质选择 **一个** 维度并保持一致：

| 维度 | 适用场景 |
|------|---------|
| by file/module | 代码修改、重构 |
| by subsystem | 审计、横切审查 |
| by document | 文档工作 |
| by finding/item | 验证、研究、分类 |

### 3.4 反模式

| # | 反模式 | 为什么不好 |
|---|--------|-----------|
| 1 | 一个巨大 `agent()` 调用"做所有事" | 无法验证、无法恢复、prompt 不可管理 |
| 2 | 任务未指定目标时硬编码列表（如模块名） | 应先用 Agent 枚举 |
| 3 | 同一脚本中混用多个分解维度 | 如部分 span 按文件、部分按子系统——应选一个维度 |
| 4 | span 的微型工作流只有单个 agent 调用且无 phase | 那是步骤不是工作单元，用 `phase()` 代替 |

---

## 4. 原语使用指南

Maestro 在 Lua 沙箱中注册了以下原语（Lua 全局变量）。完整 API 签名见 [`sdk-reference.md`](./sdk-reference.md)，此处聚焦 **使用场景和最佳实践**。

> **同步契约**: 本节原语签名以 `LUA_DSL_REFERENCE`（planner prompt）为准。文档中 `pipeline` 的 stage 签名与 [`lua-workflow-spec.md`](./dev/lua-workflow-spec.md) 存在历史差异——以本指南和 `src/planner.rs` 为权威。

### 4.1 agent(opts) → result

**最基本的工作单元**：运行一个子 Agent 到完成。

**opts 字段**：

| 字段 | 必填 | 说明 |
|------|------|------|
| `prompt` | ✓ | 子 agent 的指令文本 |
| `schema` |  | JSON Schema（强烈推荐；按字段访问 `r.output.xxx` 时**必须**） |
| `model` |  | 覆盖默认 model（不传则用 backend 默认） |
| `name` |  | 短标识符，CLI 显示（如 `"analyze-auth"`） |
| `description` |  | 一行说明，CLI 显示（如 `"审查 auth 模块安全"`） |
| `timeout_ms` |  | 单 agent 超时（毫秒） |

```lua
local r = agent({ prompt = "...", schema = MY_SCHEMA })
if not r.ok then
  log("agent 失败: " .. (r.status or "unknown"), "warn")
  -- 决策：跳过、重试、或 report() 中止
end
local data = r.output   -- 有 schema 时安全访问
```

**返回值结构**：

| 字段 | 类型 | 说明 |
|------|------|------|
| `ok` | boolean | `true` 表示成功 |
| `status` | string | `"ok"` / `"error"` / `"cancelled"` / `"timed_out"` |
| `output` | table | Agent 响应（JSON → Lua table） |
| `tokens` | int | token 用量 |
| `findings` | array | 累积的 findings（如有） |

#### Schema：强烈推荐

当你要按字段名访问 `r.output.xxx` 时，**必须** 提供 schema。没有 schema，输出是自由文本，可能不是合法 JSON，字段访问会静默返回 `nil` 并破坏下游逻辑。

```lua
-- ✅ 在脚本顶部定义命名 schema table，复用
local FINDINGS = {
  type = "object",
  properties = {
    files = { type = "array",
              items = { type = "object",
                        properties = { path = { type = "string" },
                                       purpose = { type = "string" } },
                        required = { "path", "purpose" } } },
    summary = { type = "string" }
  },
  required = { "files", "summary" }
}

-- 然后使用
local r = agent({ prompt = "...", schema = FINDINGS })
-- r.output.files 此时保证可用
```

提供 schema 后，runtime 会强制结构化输出、验证、并在不匹配时自动重试。

### 4.2 parallel(items, mapFn) → array\<result\>

**栅栏并行**：所有 item 并发执行，等待全部完成后返回。

```lua
local results = parallel(urls, function(url)
  return { prompt = "抓取并总结: " .. url, schema = SUMMARY }
end)
-- results[i] 对应 urls[i]，保序
```

**使用场景**：需要 **所有** 结果才能继续时（如收集 → 综合分析）。

`mapFn` 必须返回一个 agent opts table（与 `agent()` 相同的形状）。

### 4.3 pipeline{ items=, stages=, max_inflight= } → table

**流式多阶段**：每个 item 独立流过所有阶段；不同 item 可同时处于不同阶段。**默认优先使用 pipeline 而非 parallel。**

> **关键区别**：与 `parallel()` 不同，pipeline 的 stage handler **不会**被自动执行。handler 内部必须**自行调用 `agent()`** 并返回其 result；handler 的返回值直接成为下一 stage 的入参。

stage 有两种形式：

| 形式 | 写法 | stage 标签 |
|------|------|-----------|
| 函数 | `function(prev) ... end` | 自动 `stage_N` |
| 命名 table | `{ label = "...", handler = function(prev) ... end }` | 自定义 |

> ⚠️ table 形式字段是 `label` / `handler`（不是 `name`）。

```lua
local results = pipeline{
  items = modules,
  max_inflight = 4,
  stages = {
    function(mod)
      phase("analyze " .. mod.name)
      return agent({ prompt = "分析 " .. mod.path, schema = ANALYSIS })
    end,
    function(prev)
      phase("assess " .. (prev.output and prev.output.module or "?"))
      if not prev.ok then
        return { ok = false, output = { module = "unknown", score = 0 } }
      end
      return agent({ prompt = "评估: " .. json.encode(prev.output), schema = ASSESS })
    end
  }
}
```

#### Stage 数据流

```
Stage 1:  handler(item)     → [内部调用 agent()] → 返回值(data₁)
Stage 2:  handler(data₁)    → [内部调用 agent()] → 返回值(data₂)
Stage 3:  handler(data₂)    → [内部调用 agent()] → 返回值(data₃)
...
```

- 每个 `handler(prev)` 接收上一阶段的 **返回值**（Stage 1 接收原始 item）
- handler 内部调用 `agent()`，将 result（或自定义数据）作为返回值
- 返回值直接传递给下一 stage——runtime **不会**自动执行 agent
- `pipeline_result.items[i].output` 是 item i 最后一个 stage 的返回值
- `pipeline_result.items[i].stages[j]` 是各阶段统计 `{ label, status, elapsed_ms }`

#### 错误降级

如果某阶段返回的 result 失败（`prev.ok = false`），下一阶段 **仍然会收到** 该返回值。在 handler 开头检查 `prev.ok` 并决定：优雅降级还是中止。降级时直接返回默认数据，**不** 调用 agent。

```lua
function(prev)
  if not prev.ok then
    -- 降级：直接返回默认数据（不调用 agent）
    return { ok = false, output = { module = "unknown", score = 0 } }
  end
  return agent({ prompt = "处理: " .. json.encode(prev.output), schema = SCHEMA })
end
```

#### 返回值结构

```lua
local res = pipeline{ items = ..., stages = ..., max_inflight = 4 }
-- res.items[i] = {
--   index  = 0,          -- 原始 item 索引（0-based）
--   output = <data>,     -- 最后一个 stage 的返回值
--   stages = [           -- 各阶段执行统计
--     { label = "scan",     status = "Ok",     elapsed_ms = 120 },
--     { label = "classify", status = "Ok",     elapsed_ms = 85  },
--   ],
-- }
-- res.ok              = 3    -- 成功 item 数
-- res.failed          = 0    -- 失败 item 数（任一阶段失败即计）
-- res.total_stages    = 2
-- res.total_elapsed_ms = 820
```

### 4.4 phase(name, planned?) → phase_id

声明一个进度阶段。用于 span 内部的单步，或扁平工作流。

```lua
phase("analyze", 1)
local r = agent({ prompt = "...", schema = S })
```

### 4.5 phase_begin(name, planned?) / phase_end(span_id?)

**结构性 span**：与 `phase()` 不同，span 可以嵌套、支持 resume。每个 `phase_begin()` **必须** 配对一个 `phase_end()`。

```lua
-- 2 层嵌套（默认）：外层 span + 内层 phase 步骤
local span = phase_begin("review module-A")
  phase("analyze")  → phase("report")
phase_end(span)
```

#### 嵌套层级指导

| 层级 | 适用场景 | 结构 |
|------|---------|------|
| **2 层**（默认） | 单模块/单文件工作 | 外 span + 内 `phase()` 步骤 |
| **3 层**（大范围） | 整 crate / monorepo | group span + module span + 内步骤 |

```lua
-- 3 层嵌套：subsystem → module → steps
phase_begin("review subsystem")
  phase_begin("review module-A")
    phase("analyze") → phase("assess")
  phase_end()
  phase_begin("review module-B")
    phase("analyze") → phase("assess")
  phase_end()
phase_end()
```

### 4.6 report(value)

设置工作流最终输出并结束运行。**必须调用，且只调用一次**——第一次调用生效，后续调用被忽略。

```lua
report({ refactored = 3, results = results })
```

> **关键**：在错误路径中调用 `report()` 后 **必须** `return`，防止继续执行 nil 解引用代码。

### 4.7 其他原语

| 原语 | 签名 | 说明 |
|------|------|------|
| `log(msg, level?)` | `level`: `"info"`(默认) / `"warn"` / `"error"` | 输出状态行，CLI 和事件日志可见 |
| `budget(time_ms?, max_rounds?)` | 可选 | 提示资源限制（当前仅通知性） |
| `workflow(path, args?)` | → result | 调用另一个已保存的 `.lua` workflow 作为子步骤 |
| `json.encode(v)` / `json.decode(s)` | — | JSON 序列化辅助，用于 Agent prompt 间传递结构化数据 |

---

## 5. 全局变量与环境

脚本可访问以下预注入全局变量：

| 全局 | 类型 | 说明 |
|------|------|------|
| `args` | table | 用户通过 `--args JSON` 传入的参数；如 `args.topic` |
| `ctx` | table | 运行上下文；`ctx.run_id` 是当前运行 ID（字符串） |
| `completed_spans` | table / nil | **仅 resume 模式** 非空；键为已完成 span 名称。见 [§6](#6-resume-模式) |

---

## 6. Resume 模式

当 runtime 恢复一个之前中断的运行时，`completed_spans` 全局变量为非 nil。它是一个 table，键是已完成的 span 名称。

**规则**：在每个 `phase_begin()` 调用前检查 `completed_spans`，跳过已完成的 span。

### Skip Pattern（固定惯用法）

在每个循环体的开头使用这个确切模式：

```lua
for _, item in ipairs(items) do
  local name = "review " .. item.name
  if completed_spans and completed_spans[name] then
    log("跳过已完成: " .. name)
    goto continue
  end
  local span = phase_begin(name)
    -- ... 工作内容 ...
  phase_end(span)
  ::continue::
end
```

> **要点**：span 名称必须与 `completed_spans` 的键匹配。保持命名一致（如始终用 `"review " .. item.name`）是 resume 正确工作的前提。

---

## 7. 错误处理与降级

### 7.1 三步法则

1. **检查** `result.ok` 后再使用 `result.output`
2. **失败时**：`log()` 记录错误，然后决策——跳过、重试或 `report()` 中止
3. **中止时**：`report()` 后立即 `return`，防止 nil 解引用

```lua
local r = agent({ prompt = "...", schema = S })
if not r.ok then
  log("agent 失败: " .. (r.status or "unknown"), "warn")
  report({ error = r.status })
  return   -- ← 必须 return
end
-- 安全使用 r.output
```

### 7.2 优雅降级

在 pipeline 中，当某阶段失败时，向下一阶段喂一个最小/默认 prompt，而不是让管线崩溃：

```lua
function(prev)
  if not prev.ok then
    return { prompt = "返回最小默认值", schema = SCHEMA }
  end
  return { prompt = "处理: " .. json.encode(prev.output), schema = SCHEMA }
end
```

### 7.3 常见错误场景

| 场景 | 后果 | 正确做法 |
|------|------|---------|
| 不检查 `r.ok` 直接访问 `r.output` | 失败时 nil 解引用 | 先 `if not r.ok then` |
| `report()` 后不 `return` | 继续执行 nil 解引用代码 | `report({...}); return` |
| 不提供 schema 但按字段访问 output | 静默返回 nil | 定义并传入 schema table |

---

## 8. 对抗性验证模式

当任务需要交叉验证/核实结果时，用 Lua 手动实现对抗性验证循环。**这是模式而非原语**——只在任务真正需要交叉检查时使用。

### 8.1 五步流程

```
1. PRODUCE:   用 parallel() 对每个 item 运行 producer Agent，生成 findings
2. CHALLENGE: 对每条 finding 运行 adversary Agent，尝试反驳
3. VOTE:      只保留 approval rate >= 阈值（如 0.7）的 findings
4. ITERATE:   将幸存 findings 作为新 items，重复最多 N 轮
5. STOP:      收敛（无 finding 被反驳）或达到最大轮次时停止
```

### 8.2 Lua 骨架

```lua
local items = gather.output.findings or {}
local max_rounds = 3
local threshold = 0.7

local VOTE_SCHEMA = {
  type = "object",
  properties = { approve = { type = "boolean" } },
  required = { "approve" }
}

for round = 1, max_rounds do
  log("对抗轮次 " .. round)
  local votes = parallel(items, function(finding)
    return {
      prompt = "评估此 finding 的准确性。\n" .. json.encode(finding),
      schema = VOTE_SCHEMA
    }
  end)
  local survivors = {}
  for i, finding in ipairs(items) do
    if votes[i].ok and votes[i].output.approve then
      table.insert(survivors, finding)
    end
  end
  if #survivors == #items then break end   -- 收敛
  items = survivors
end
```

---

## 9. 编写规则速查

以下是 planner prompt 中定义的 17 条规则，按类别整理：

### 脚本结构

| # | 规则 |
|---|------|
| 1 | 脚本 **必须** 以工作流架构注释头部开头（[§2](#2-工作流架构注释)），之后立即声明 `meta` 表 |
| 2 | 脚本 **必须** 定义 `function main()` 入口，所有执行代码（agent/phase/report）放在 main() 内部 |
| 18 | 脚本 **必须** 以 `report(<table>)` 结束（在 main() 内部） |
| 11 | 只输出一个 ` ```lua ` 代码块——无解释文字 |

### 安全边界

| # | 规则 |
|---|------|
| 3 | 脚本不碰文件系统/shell——告诉 Agent 做什么 |
| 4 | 扇出有界——最多 ~16 并发 Agent。大集合先让 Agent 枚举/分块 |

### 原语选择

| # | 规则 |
|---|------|
| 5 | 优先 `pipeline()`；`parallel()` 仅在需要全部结果时用 |
| 6 | **始终** 检查 `result.ok` 后再使用 `result.output` |
| 7 | **始终** 为需要按字段访问输出的 `agent()`/`parallel()`/`pipeline()` 调用提供 `schema` |
| 10 | 用 `phase()` / `log()` 让进度可读 |

### 错误处理

| # | 规则 |
|---|------|
| 8 | 错误 `report()` 后 **必须** `return` |
| 9 | `report()` 只调用一次——首次调用生效 |

### 任务分解

| # | 规则 |
|---|------|
| 13 | 大任务分解为 phase span，每个 span 内部复用相似工作流 |
| 14 | 未知范围先让 Agent 枚举目标，再循环——不硬编码 |
| 15 | `phase_begin()` **必须** 配对 `phase_end()` |
| 16 | Span 可嵌套 2–3 层；默认 2 层，整 crate/monorepo 用 3 层 |

### Resume

| # | 规则 |
|---|------|
| 17 | Resume 模式下（`completed_spans` 非 nil），用 `goto continue` 跳过已完成 span |

### 字符串规范

| # | 规则 |
|---|------|
| 12 | **始终** 用双引号包裹字符串值——尤其是非 ASCII 文本（中文、日文等）。Lua 标识符仅限 ASCII，裸 CJK 字符在引号外是语法错误 |

  - ❌ `prompt = 整理文档`（语法错误）
  - ✅ `prompt = "整理文档"`

  适用于 table 字段、函数参数和字符串拼接操作数。

---

## 10. 完整示例

### 10.1 按模块重构（静态分解）

已知模块列表，每个模块走 `analyze → refactor → verify` 三步。

```lua
--------------------------------------------
-- Goal:  Refactor auth, db, api modules
-- Arch:
--   for each module in {auth, db, api}:
--     +-- analyze  --> [ANALYSIS]
--     +-- refactor --> [CHANGES]
--     \-- verify   --> [VERIFY]
-- Flow:  {modules} -> ANALYSIS -> CHANGES -> VERIFY -> report
--------------------------------------------
local MODULES = { "auth", "db", "api" }
local results = {}

for _, mod in ipairs(MODULES) do
  local name = "refactor " .. mod
  if completed_spans and completed_spans[name] then
    log("跳过已完成: " .. name)
    goto continue
  end
  local m = phase_begin(name)
    phase("analyze")
    local a = agent({ prompt = "分析 " .. mod .. " 的问题", schema = ANALYSIS })

    phase("refactor")
    local c = agent({ prompt = "对 " .. mod .. " 应用重构", schema = CHANGES })

    phase("verify")
    local v = agent({ prompt = "验证 " .. mod .. " 仍通过测试", schema = VERIFY })
    table.insert(results, { module = mod, ok = v.ok })
  phase_end(m)
  ::continue::
end

report({ refactored = #results, results = results })
```

### 10.2 整 crate 重构（动态枚举，3 层嵌套）

范围未知——先发现子系统，再发现模块，最后逐模块重构。

```lua
--------------------------------------------
-- Goal:  Refactor entire crate by subsystem
-- Arch:
--   discover subsystems                     --> [subsystems[]]
--   for each subsystem:
--     +-- discover modules                  --> [modules[]]
--     +-- for each module:
--           +-- analyze                     --> [ANALYSIS]
--           +-- change                      --> [CHANGES]
--           \-- verify                      --> [VERIFY]
-- Flow:  discover -> subsystems[] -> modules[] -> changes -> report
--------------------------------------------
phase("discover subsystems")
local discover = agent({
  prompt = "枚举 src/ 下需要重构的子系统",
  schema = SUBSYSTEMS_SCHEMA
})

for _, sys in ipairs(discover.output.subsystems or {}) do
  local gname = "refactor " .. sys.name
  if completed_spans and completed_spans[gname] then
    goto skip_sys
  end
  local g = phase_begin(gname)
    local mods = agent({
      prompt = "列出 " .. sys.path .. " 中需要修改的模块",
      schema = MODULES_SCHEMA
    })
    for _, mod in ipairs(mods.output.modules or {}) do
      local mname = "refactor " .. mod.name
      if completed_spans and completed_spans[mname] then
        goto skip_mod
      end
      local m = phase_begin(mname)
        phase("analyze")
        phase("change")
        phase("verify")
      phase_end(m)
      ::skip_mod::
    end
  phase_end(g)
  ::skip_sys::
end

report({ done = true })
```

### 10.3 简单研究工作流

收集来源 → 并行分析每个来源。

```lua
--------------------------------------------
-- Goal:  Research a topic and analyze sources
-- Arch:
--   gather sources (agent)                  --> [sources[]]
--   analyze each (parallel)                 --> [ANALYSIS[]]
-- Flow:  gather -> sources[] -> parallel(analyze) -> report
--------------------------------------------
phase("research", 1)

local topic = args.topic or "AI safety"

local SOURCES_SCHEMA = {
  type = "object",
  properties = {
    sources = { type = "array", items = {
      type = "object",
      properties = { title = { type = "string" }, url = { type = "string" }, summary = { type = "string" } },
      required = { "title", "summary" }
    } }
  },
  required = { "sources" }
}

local ANALYSIS_SCHEMA = {
  type = "object",
  properties = {
    insights = { type = "array", items = { type = "string" } },
    credibility = { type = "string" }
  },
  required = { "insights" }
}

local gather = agent({
  prompt = "研究: " .. topic,
  schema = SOURCES_SCHEMA
})
if not gather.ok then
  report({ error = "gather 失败: " .. gather.status })
  return
end

local results = parallel(gather.output.sources or {}, function(src)
  return {
    prompt = "分析此来源并提取关键洞察。\n" .. json.encode(src),
    schema = ANALYSIS_SCHEMA
  }
end)

report({ topic = topic, sources = #results, results = results })
```

### 10.4 对抗性验证片段

多轮投票交叉检查 findings（在需要时添加）。

```lua
--------------------------------------------
-- Goal:  Cross-check findings via voting
-- Arch:
--   repeat (<= N rounds):
--     +-- vote on findings (parallel)       --> [votes[]]
--     +-- keep survivors                    --> [survivors[]]
--     \-- break if converged
-- Flow:  findings -> vote -> survivors -> (loop) -> report
--------------------------------------------
local items = gather.output.findings or {}
local max_rounds = 3
local threshold = 0.7

local VOTE_SCHEMA = {
  type = "object",
  properties = { approve = { type = "boolean" } },
  required = { "approve" }
}

for round = 1, max_rounds do
  log("对抗轮次 " .. round)
  local votes = parallel(items, function(finding)
    return {
      prompt = "评估此 finding 的准确性。\n" .. json.encode(finding),
      schema = VOTE_SCHEMA
    }
  end)
  local survivors = {}
  for i, finding in ipairs(items) do
    if votes[i].ok and votes[i].output.approve then
      table.insert(survivors, finding)
    end
  end
  if #survivors == #items then break end
  items = survivors
end
```

---

## 附录：与现有文档的关系

| 文档 | 定位 | 与本指南的关系 |
|------|------|---------------|
| **本指南** | 实践方法论 | 蓝本：`LUA_DSL_REFERENCE`。聚焦"怎么设计、怎么想" |
| [`lua-workflow-spec.md`](./dev/lua-workflow-spec.md) | 技术规范 | 精确 API 定义、文件格式、验证规则。聚焦"是什么、怎么写" |
| [`sdk-reference.md`](./sdk-reference.md) | API 参考 | 原语快速查阅 |
| [`architecture/planner.md`](./architecture/planner.md) | 模块架构 | planner 内部实现（prompt 构造、校验、重试） |
| [`architecture/runtime.md`](./architecture/runtime.md) | 模块架构 | 沙箱与执行流程 |

### 已知差异

本指南与 [`lua-workflow-spec.md`](./dev/lua-workflow-spec.md) 之间的历史差异：

| 差异点 | 本指南（对齐运行时） | lua-workflow-spec.md |
|--------|-------------------|---------------------|
| pipeline stage 签名 | 函数或 `{ label=, handler= }`（两种均支持） | `{ name=, handler= }` — 字段名不同 |
| 脚本结构 | 直接脚本（planner 生成物） | `meta = {...}` + `function main() ... end` |
| converge | 作为 Lua 模式实现（非原语） | 作为内置原语 `converge()` |

> **权威声明**：当任何文档与实现冲突时，以 `src/runtime/pipeline.rs` `register_pipeline_sdk()` 和 `src/runtime/sandbox.rs` `register_sdk()` 为最终真相。pipeline 的 stage handler 必须自行调用 `agent()`（返回值传递给下一 stage，runtime 不自动执行 agent）；table 形式字段为 `label` / `handler`。
