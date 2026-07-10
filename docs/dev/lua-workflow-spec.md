# Lua Workflow 文件技术规范

> **版本**: v2.0.0  
> **状态**: 正式规范  
> **上次更新**: 2025-08-19  
> **规范依据**: `src/runtime/sandbox.rs` `register_sdk()` (真相源) + `src/planner.rs` `LUA_DSL_REFERENCE` (Agent prompt 契约)

---

## 1. 文件格式

### 1.1 编码与扩展名

- **编码**: UTF-8 (无 BOM)
- **扩展名**: `.lua`
- **换行符**: LF (`\n`)

### 1.2 文件结构

一个合法的 Luft Lua Workflow 文件由三部分组成：

```
[meta 声明]           — 必填，纯 Lua table，描述 phase 结构
[main() 函数]         — 必填，编排逻辑入口
```

**最小合法文件**：

```lua
meta = { phases = {} }

function main()
    report({ ok = true })
end
```

### 1.3 Meta 声明

Meta 是顶层的纯数据声明——一个 Lua table，描述工作流的 phase 结构（供进度展示）。须在所有函数调用之前赋值。

```lua
meta = {
    phases = {
        {
            label = "discovery",
            detail = "扫描代码库中的函数定义",
            agents = 3,
            depends_on = {}
        },
        {
            label = "analysis",
            detail = "分析每个函数的使用情况",
            agents = 5,
            depends_on = { 0 }
        }
    },
    reasoning = "先发现所有函数再分析使用情况"
}
```

**字段约束**：

| 字段 | Lua 类型 | 必填 | 说明 |
|------|----------|------|------|
| `phases` | `table` (array) | ✅ | 顶层 phase 列表 |
| `phases[i].label` | `string` | ✅ | 显示名，须与 `phase()` 调用对应 |
| `phases[i].detail` | `string` | ✅ | 一行描述 |
| `phases[i].agents` | `integer` | 否 (默认 `0`) | 预计启动的 agent 数量 |
| `phases[i].depends_on` | `table` (array) | 否 (默认 `{}`) | 依赖的 phase 索引（0-based） |
| `reasoning` | `string` | 否 (默认 `""`) | 规划理由 |

**提取方式**（Planner）：

```rust
fn extract_meta(script: &str) -> Option<PlanMeta> {
    let lua = mlua::Lua::new();
    // 只执行顶层赋值（meta = {...} + function main() ... end），
    // 不调用 main()，因此不会执行任何 agent() 调用。
    lua.load(script).exec().ok()?;
    let meta_table: mlua::Table = lua.globals().get("meta").ok()?;
    Some(PlanMeta {
        phases: meta_table.get("phases").ok()?,
        reasoning: meta_table.get::<_, Option<String>>("reasoning")
            .ok()?.unwrap_or_default(),
    })
}
```

**关键设计**：
- `meta = {...}` 是纯数据定义，无副作用
- `function main() ... end` 同样是定义，不产生调用
- Planner 执行顶层代码安全且廉价（仅几次赋值和函数注册）
- 与 Claude Code Dynamic Workflows `meta = {...}` 模式一致

---

## 2. 沙箱模型

### 2.1 禁用的 Lua 全局

脚本运行在受限沙箱中。以下标准 Lua 全局被置为 `nil`（`src/runtime/sandbox.rs:715-721`）：

| 禁用全局 | 原因 |
|----------|------|
| `io` | 禁止文件/标准流操作 |
| `os` | 禁止系统调用 (`execute`, `getenv` 等) |
| `debug` | 禁止调试接口 |
| `package` | 禁止模块加载 |
| `require` | 同上 |
| `loadfile` | 禁止动态加载文件 |
| `dofile` | 同上 |
| `loadstring` | 禁止动态字符串编译 |

### 2.2 执行模型

- **读/写分离**：顶层（`meta = {...}`、`function main() ... end`）是声明区，纯数据定义；`main()` 是编排区，含所有 agent 调用
- **脚本是编排器，不是执行器**：脚本持有循环、分支和中间结果，所有实际工作由生成的子 Agent 完成
- **脚本无工具**：Agent 有工具（文件读写、grep、shell 等），脚本没有
- **脚本无网络/文件系统访问**：任何数据获取都必须通过 Agent
- **双重安全**：加载脚本只定义 `main()`；Runtime 单独调用 `main()` 才执行编排

### 2.3 资源限制

| 限制 | 机制 | 状态 |
|------|------|------|
| 内存 | mlua 内存限制 | ✅ 强制执行 |
| 指令计数 | `set_hook` 检测 | ⚠️ 检测但未强制终止 |
| 运行时间 | `budget()` 设置 | ⚠️ 通知但未强制终止 |
| Fan-out 上限 | ~16 并发 Agent | 建议性约束 |

---

## 3. SDK 原语

脚本可调用 10 个 SDK 原语。所有原语均为 Lua 全局变量。**原语仅能在 `main()` 及其调用的函数内调用，顶层声明区不应调用任何原语。**

### 3.1 `agent(opts) -> result`

执行单个 Agent 任务。

**参数 `opts`** (`table`):

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `prompt` | `string` | ✅ | Agent 的任务提示词 |
| `model` | `string` | 否 | 指定模型（如 `"gpt-4"`） |
| `schema` | `table` | 否 | JSON Schema，约束 Agent 输出结构 |
| `backend` | `string` | 否 | 指定后端 |
| `timeout_ms` | `integer` | 否 | 超时时间（毫秒） |

**返回值 `result`** (`table`):

| 字段 | 类型 | 说明 |
|------|------|------|
| `status` | `string` | `"success"` 或 `"error"` |
| `ok` | `boolean` | `true` 当 status 为 success |
| `output` | `any` | Agent 的输出值（通常为 JSON 值） |
| `tokens` | `integer` | 消耗的 token 总数 |
| `findings` | `array` | Agent 报告的 findings 列表 |

**缓存行为**: 当工作流以 `--resume` 恢复时，已完成 Agent 基于 blake3 hash（`prompt + model + phase_id`）跳过重执行，直接返回缓存结果。

```lua
function main()
    local result = agent({
        prompt = "分析这段代码的安全风险",
        model = "gpt-4",
        schema = { findings = "array" }
    })
    if result.ok then
        log("agent 完成，tokens: " .. result.tokens)
    end
    report(result)
end
```

### 3.2 `parallel(items, mapFn) -> array<result>`

栅栏并行：所有任务并发执行，完成后统一返回。结果保持输入顺序。

**参数**:

| 参数 | 类型 | 说明 |
|------|------|------|
| `items` | `array` | 输入 item 列表 |
| `mapFn` | `function` | `function(item) -> agent_opts_table`，必须返回 agent opts table |

**返回值**: `array<table>`，每个元素与 `agent()` 返回值结构相同。

**约束**: `mapFn` 必须且仅返回 agent opts table，不能返回其他类型，否则运行时错误。

```lua
function main()
    local files = { "src/a.rs", "src/b.rs", "src/c.rs" }
    local results = parallel(files, function(file)
        return { prompt = "审查安全问题: " .. file }
    end)
    -- results[1] 对应 files[1]，结果保序
    report(results)
end
```

### 3.3 `pipeline{ items, stages, max_inflight? } -> table`

多阶段流式管道。每个 item 独立通过所有阶段，不同 item 可在不同阶段并发执行（非栅栏）。

**参数** (`table`):

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `items` | `array` | ✅ | 输入 item 列表 |
| `stages` | `array` | ✅ | 阶段定义列表 |
| `max_inflight` | `integer` | 否 | 最大并发 item 数 |

**`stages` 元素** (`table`):

| 字段 | 类型 | 说明 |
|------|------|------|
| `name` | `string` | 阶段名称 |
| `handler` | `function` | `function(item, prev_result) -> result` |

**返回值** (`table`):

| 字段 | 类型 | 说明 |
|------|------|------|
| `ok` | `integer` | 成功的 item 数 |
| `failed` | `integer` | 失败的 item 数 |
| `total_stages` | `integer` | 总阶段执行次数 |
| `total_elapsed_ms` | `integer` | 总耗时（毫秒） |
| `items` | `array` | 每个 item 的各阶段结果详情 |

```lua
function main()
    local result = pipeline{
        items = { "topic-A", "topic-B", "topic-C" },
        max_inflight = 4,
        stages = {
            { name = "research", handler = function(item)
                return agent({ prompt = "研究: " .. item })
            end },
            { name = "summarize", handler = function(item, prev)
                return agent({ prompt = "总结: " .. json.encode(prev.output) })
            end },
        }
    }
    report(result)
end
```

### 3.4 `converge(items, opts?) -> table`

对抗性收敛验证。Producer Agent 生成 findings → Adversary Agent 反驳 → 投票决定幸存者 → 重复直到收敛。

**参数**:

| 参数 | 类型 | 说明 |
|------|------|------|
| `items` | `array` | 待验证的 item 列表 |
| `opts` | `table` | 可选配置 |

**`opts` 字段**:

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `adversarial` | `boolean` | `true` | 是否启用对抗验证 |
| `vote_threshold` | `number` | `0.7` | 投票通过阈值 (0.0–1.0) |
| `max_rounds` | `integer` | `3` | 最大验证轮次 |
| `producers_per_item` | `integer` | `1` | 每个 item 的 producer 数量 |
| `adversaries_per_finding` | `integer` | `1` | 每个 finding 的 adversary 数量 |
| `model` | `string` | `nil` | Agent 使用的模型 |

**返回值** (`table`):

| 字段 | 类型 | 说明 |
|------|------|------|
| `converged` | `boolean` | 是否收敛 |
| `rounds` | `integer` | 实际执行轮次 |
| `findings` | `array` | 最终幸存 findings |
| `round_stats` | `array` | 每轮统计 (`items/findings/surviving`) |

```lua
function main()
    local claims = { "密码使用 bcrypt", "API 有 RBAC", "输入防 SQL 注入" }
    local result = converge(claims, {
        adversarial = true,
        vote_threshold = 0.7,
        max_rounds = 3
    })
    if result.converged then
        log("收敛，幸存 " .. #result.findings .. " 条")
    end
    report(result)
end
```

### 3.5 `phase(name, planned?) -> phase_id`

进度分组。将后续 Agent 调用归入指定阶段，用于进度追踪。

**参数**:

| 参数 | 类型 | 说明 |
|------|------|------|
| `name` | `string` | 阶段显示名 |
| `planned` | `integer` | 预计 agent 数量（可选，进度展示用） |

**返回值**: `phase_id` (`integer`)，从 0 自增。

```lua
function main()
    local pid = phase("研究阶段", 5)
    -- 后续 agent()/parallel()/pipeline() 调用均归属此阶段
end
```

### 3.6 `log(msg, level?)`

输出结构化日志事件。

**参数**:

| 参数 | 类型 | 说明 |
|------|------|------|
| `msg` | `string` | 日志消息 |
| `level` | `string` | `"info"` / `"warn"` / `"error"`（默认 `"info"`） |

```lua
function main()
    log("开始处理", "info")
    log("文件不存在: " .. path, "warn")
end
```

### 3.7 `budget(time_ms?, max_rounds?)`

提示运行时限制（通知性，非强制性）。

| 参数 | 类型 | 说明 |
|------|------|------|
| `time_ms` | `integer` | 时间限制（毫秒） |
| `max_rounds` | `integer` | 最大轮次限制 |

### 3.8 `workflow(path, args?) -> value`

嵌套子工作流。加载并执行另一个 Lua 脚本。子工作流共享全局并发 cap。

**参数**:

| 参数 | 类型 | 说明 |
|------|------|------|
| `path` | `string` | 子工作流文件路径 |
| `args` | `table` | 传递给子工作流的参数 |

**返回值**: 子工作流中 `report()` 的值。

```lua
function main()
    local result = workflow("~/workflows/deep-research.lua", {
        topic = "Rust 异步运行时对比"
    })
    report(result)
end
```

### 3.9 `report(value)`

设置工作流最终输出。**每个脚本的 `main()` 必须在末尾调用一次。**

**参数**:

| 参数 | 类型 | 说明 |
|------|------|------|
| `value` | `any` | 任意 Lua 值，会被序列化为 JSON 输出 |

```lua
function main()
    report({
        status = "complete",
        findings = results,
        summary = "发现 3 个安全问题"
    })
end
```

### 3.10 `json.encode(t)` / `json.decode(s)`

序列化辅助。

```lua
local s = json.encode({ key = "value" })  -- '{"key":"value"}'
local t = json.decode(s)                  -- { key = "value" }
```

---

## 4. 验证规则

脚本在进入 Runtime 前经过两层验证：

| # | 规则 | 级别 | 实现 |
|---|------|------|------|
| 1 | 脚本必须是合法的 Lua 语法 | **ERROR** | `mlua::Lua::load()` 编译 |
| 2 | 脚本必须定义 `main()` 函数 | **ERROR** | `mlua` 类型检查 `Function` |
| 3 | 脚本必须调用 `report(...)` | **ERROR** | 字符串包含检查 `"report("` |
| 4 | meta phase label 应在 script 中有对应 `phase()` 调用 | ⚠️ WARN | 交叉验证 |
| 5 | meta phases 不应为空 | ⚠️ WARN | 合理性检查 |
| 6 | 并发 agent 不应超过 32 | ⚠️ WARN | fan-out 检查 |

---

## 5. Agent Task 选项规范

所有生成 Agent 的原语（`agent`/`parallel`/`pipeline`/`converge`）共享以下 opts 规范：

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `prompt` | `string` | — | **(必填)** Agent 的自然语言任务描述 |
| `model` | `string` | 系统默认 | 覆盖全局模型选择 |
| `schema` | `table` | 无 | JSON Schema 约束 Agent 输出格式 |
| `backend` | `string` | 系统默认 | 指定 Agent 后端 |
| `timeout_ms` | `integer` | 无超时 | Agent 执行超时（毫秒） |

### 5.1 Schema 约束

当指定 `schema` 时，Agent 输出必须符合该 JSON Schema。违规视为任务失败。

```lua
function main()
    local result = agent({
        prompt = "列出仓库中的 .rs 文件",
        schema = {
            type = "object",
            required = { "files" },
            properties = {
                files = {
                    type = "array",
                    items = { type = "string" }
                }
            }
        }
    })
    -- result.output.files 保证是 string[]
end
```

### 5.2 缓存键

缓存键基于 `blake3(prompt + model + phase_id)` 计算。相同缓存键的 Agent 在 `--resume` 模式下跳过执行。

---

## 6. 执行生命周期

```
┌──────────────────┐
│ 1. 加载 .lua     │   mlua::Lua::load() — 仅编译，不执行
│    (compile)     │
└────────┬─────────┘
         │
┌────────▼─────────┐
│ 2. 语法验证       │   编译失败 → 拒绝
│   (load/compile) │
└────────┬─────────┘
         │ 通过
┌────────▼─────────┐
│ 3. 执行顶层       │   运行 meta = {...} + function main() ... end
│   (exec top)     │   ★ 此时 Planner 提取 meta — 安全，无 agent 调用
└────────┬─────────┘
         │
┌────────▼─────────┐
│ 4. 沙箱注入       │   禁用: io, os, debug, require, load*
│   (apply_sandbox)│   注入: 10 SDK globals
└────────┬─────────┘
         │
┌────────▼─────────┐
│ 5. 调用 main()    │   ★ 此时才执行 agent()/parallel()/...
│   (execute)      │   Agent 调用通过 Handle::block_on 阻塞等待
└────────┬─────────┘
         │
┌────────▼─────────┐
│ 6. report()      │   值序列化为 JSON → report_sink
│   触发            │
└────────┬─────────┘
         │
┌────────▼─────────┐
│ 7. 返回           │   结果写入 Journal (用于 --resume)
│   完成            │
└──────────────────┘
```

**关键语义**:
- 步骤 3 是 Planner 提取 meta 的时机——此时无副作用
- 步骤 5 是 Runtime 真正执行编排的时机
- `main()` 内是同步语义，但 Agent 调用通过 `Handle::block_on` 阻塞等待异步结果
- 并发仅在 `parallel()`/`pipeline()`/`converge()` 内部发生

---

## 7. 常见模式

### 7.1 Fan-out（栅栏并行）

```lua
meta = {
    phases = {
        { label = "discovery", detail = "列出所有 .rs 文件", agents = 1, depends_on = {} },
        { label = "audit", detail = "并行审查每个文件", agents = 10, depends_on = { 0 } }
    },
    reasoning = "先发现再并行审查"
}

function main()
    phase("discovery", 1)
    local files = agent({ prompt = "列出所有 .rs 文件", schema = { files = "array" } })

    phase("audit", #files.output.files)
    local results = parallel(files.output.files, function(path)
        return { prompt = "审查: " .. path, schema = { issues = "array" } }
    end)

    report({ audited = #results, files = results })
end
```

### 7.2 Pipeline（流式处理）

```lua
meta = {
    phases = {
        { label = "research", detail = "深度研究每个主题", agents = 3, depends_on = {} },
        { label = "summarize", detail = "总结研究成果", agents = 3, depends_on = { 0 } }
    },
    reasoning = "先研究再总结，流水线处理"
}

function main()
    local result = pipeline{
        items = { "A", "B", "C" },
        max_inflight = 4,
        stages = {
            { name = "research", handler = function(item)
                return agent({ prompt = "研究: " .. item })
            end },
            { name = "summarize", handler = function(item, prev)
                return agent({ prompt = "总结: " .. json.encode(prev), schema = { score = "number" } })
            end },
        }
    }
    report(result)
end
```

### 7.3 Adversarial Converge（对抗验证）

```lua
meta = {
    phases = {
        { label = "propose", detail = "多角度生成方案", agents = 3, depends_on = {} },
        { label = "verify", detail = "对抗审查方案", agents = 1, depends_on = { 0 } }
    },
    reasoning = "先提出再验证，对抗收敛"
}

function main()
    phase("propose", 3)
    local proposals = parallel({ "A", "B", "C" }, function(p)
        return { prompt = "提出方案: " .. p }
    end)

    phase("verify", 1)
    local verified = converge(proposals, {
        adversarial = true,
        max_rounds = 2,
        vote_threshold = 0.6
    })

    report({ verified = verified.findings, rounds = verified.rounds })
end
```

### 7.4 嵌套工作流

```lua
meta = {
    phases = {
        { label = "sub-workflows", detail = "并行执行两个子工作流", agents = 2, depends_on = {} }
    },
    reasoning = "安全审计和性能审计并行执行"
}

function main()
    phase("sub-workflows", 2)
    local a = workflow("workflows/audit_security.lua", { target = "src/" })
    local b = workflow("workflows/audit_perf.lua", { target = "src/" })
    report({ security = a, perf = b })
end
```

---

## 8. 反模式（禁止）

| # | 反模式 | 原因 | 正确做法 |
|---|--------|------|---------|
| 1 | 顶层调用 `agent()`/`parallel()` 等原语 | 定义阶段不应有副作用 | 封装在 `main()` 内 |
| 2 | 不定义 `main()` 函数 | 验证失败 | 始终以 `function main() ... end` 封装 |
| 3 | 脚本中调用 `os.execute()` / `io.open()` | 沙箱禁用，运行时报错 | 让 Agent 执行 |
| 4 | 脚本中读取文件内容做决策 | 脚本无文件访问权限 | `agent({ prompt = "读取文件: " .. path })` |
| 5 | `parallel` 的 mapFn 返回值不是 agent opts table | 运行时报错 | `return { prompt = "..." }` |
| 6 | 不调用 `report()` 或 `report()` 不在 `main()` 内 | 验证失败，规划重试 | `main()` 末尾调用 `report(...)` |
| 7 | 在 `report()` 之后继续执行代码 | 无效果（report 是终态） | `report()` 是 `main()` 最后一个逻辑语句 |
| 8 | 超过 32 个并发 agent | Fan-out 警告 | 分批处理或用 pipeline |
| 9 | 嵌套 `parallel(parallel(...))` | 资源泄漏风险 | 用 pipeline 替代 |
| 10 | 在循环中逐个调用 `agent()` | 串行效率低 | 收集 items 后用 `parallel()` |

---

## 9. 与 Agent（Planner）的接口

Planner Agent（如 opencode）生成符合本规范的 `.lua` 文件。

**Agent 的 prompt 要求**:

1. 输出仅限一个 ` ```lua ` 代码块
2. 文件顶部声明 `meta = { phases = {...}, reasoning = "..." }`
3. 所有编排逻辑封装在 `function main() ... end` 内
4. `main()` 末尾调用 `report(...)`
5. 顶层不调用任何 SDK 原语（仅在 `main()` 内调用）
6. 脚本不包含文件系统/shell 操作
7. Fan-out 上限约 16 并发
8. 优先使用 `pipeline()`，其次 `parallel()`，验证时使用 `converge()`

**Planner 的处理流程**（`src/planner.rs:plan_workflow`）:

1. 从 Agent 响应中提取 ` ```lua ` 代码块
2. 执行顶层代码（`meta = {...}` + `function main() ... end`），通过 mlua 读取 `meta` 全局
3. 通过 `mlua::Lua::load()` 验证语法
4. 检查 `main` 是 `Function` 类型
5. 检查 `report(` 存在
6. 返回 `PlannedWorkflow { phases, script, reasoning }`

---

## 10. 附录

### A. 相关文件

| 文件 | 角色 |
|------|------|
| `src/runtime/sandbox.rs:201-551` | SDK 原语注册（真相源） |
| `src/runtime/sandbox.rs:715-721` | 沙箱禁用列表 |
| `src/planner.rs:221-292` | `LUA_DSL_REFERENCE`（Agent prompt 契约） |
| `src/planner.rs:161-200` | 响应解析（`output_to_text` → `extract_lua_block`） |
| `src/planner.rs:86-94` | `validate_generated`（语法 + `report()` 检查） |
| `docs/sdk-reference.md` | SDK 使用参考 |
| `examples/*.lua` | 示例工作流文件 |

### B. 变更历史

| 日期 | 版本 | 变更 |
|------|------|------|
| 2025-08-19 | v2.0.0 | `main()` 入口模式：`meta = {...}` Lua table（替代 `--[[@meta` 注释块）；读/写分离执行模型；mlua 安全提取 meta |
| 2025-08-19 | v1.0.0 | 初始正式规范，基于 sandbox.rs + planner.rs + sdk-reference.md |
