# Lua SDK 参考

Maestro 在 Lua 沙箱中注册了 10 个 SDK 原语，供工作流脚本调用。

## 总览

| 原语 | 用途 | 返回 |
|------|------|------|
| `agent(opts)` | 执行单个 agent 任务 | result table |
| `parallel(items, mapFn)` | 并行执行多个 agent | results array |
| `pipeline(items, stages)` | 多阶段流式处理 | pipeline result |
| `converge(items, opts?)` | 对抗性收敛验证 | converge result |
| `workflow(path, args?)` | 嵌套子工作流 | 子工作流的 report 值 |
| `phase(name, planned?)` | 进度分组 | phase_id (number) |
| `log(msg, level?)` | 结构化日志 | nil |
| `budget(time_ms?, rounds?)` | 运行时限制 | nil |
| `report(value)` | 设置工作流最终输出 | nil |
| `json.encode(t)` / `json.decode(s)` | 序列化辅助 | string / table |

---

## agent(opts)

执行单个 agent 任务。

### 参数

`opts` — table，支持以下字段：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `prompt` | string | ✅ | agent 的任务提示词 |
| `model` | string | | 指定模型（如 `"gpt-4"`） |
| `schema` | table | | JSON Schema，验证 agent 输出 |

### 返回值

table，包含：

| 字段 | 类型 | 说明 |
|------|------|------|
| `status` | string | `"success"` / `"error"` |
| `output` | any | agent 的输出（通常为 JSON 值） |
| `tokens` | number | 消耗的 token 数 |
| `findings` | array | agent 报告的 findings 列表 |

### Journal Cache

当 workflow 以 `--resume` 恢复时，已完成的 agent 会被跳过（基于 blake3 cache key = prompt + model + phase_id），直接返回缓存结果。

### 示例

```lua
local result = agent({
    prompt = "分析这段代码的安全风险",
    model = "gpt-4"
})
log("agent 完成，状态: " .. result.status)
```

---

## parallel(items, mapFn)

并行执行多个 agent 任务（栅栏同步）。所有任务完成后才返回，结果保持输入顺序。

### 参数

| 参数 | 类型 | 说明 |
|------|------|------|
| `items` | array | 输入 items 列表 |
| `mapFn` | function | `function(item) -> agent_opts`，为每个 item 构造 agent 参数 |

### 返回值

array，每个元素是一个与 `agent()` 返回值相同的 table。

### 示例

```lua
local files = { "src/a.rs", "src/b.rs", "src/c.rs" }
local results = parallel(files, function(file)
    return {
        prompt = "审查这个文件的安全问题: " .. file
    }
end)
for i, r in ipairs(results) do
    log(string.format("%s → %s", files[i], r.status))
end
```

---

## pipeline(items, stages)

多阶段流式管道。每个 item 独立通过所有阶段，不同 item 可在不同阶段并发执行（非栅栏）。

### 参数

| 参数 | 类型 | 说明 |
|------|------|------|
| `items` | array | 输入 items 列表 |
| `stages` | array | 阶段定义列表，每个阶段是一个 table：`{ name, handler }` |

`handler` 签名：`function(item, prev_result) -> result`

### 返回值

table，包含：

| 字段 | 类型 | 说明 |
|------|------|------|
| `ok` | number | 成功的 item 数 |
| `failed` | number | 失败的 item 数 |
| `total_stages` | number | 总阶段执行次数 |
| `total_elapsed_ms` | number | 总耗时（毫秒） |
| `items` | array | 每个 item 的各阶段结果 |

### 架构

```
Input → [Stage 0] → [Stage 1] → ... → [Stage N] → Output
         (worker 0)   (worker 1)         (worker N)
```

### 示例

```lua
local items = { "topic-A", "topic-B", "topic-C" }
local result = pipeline(items, {
    { name = "research", handler = function(item)
        return agent({ prompt = "研究: " .. item })
    end },
    { name = "summarize", handler = function(item, prev)
        return agent({ prompt = "总结: " .. json.encode(prev.output) })
    end },
})
log(string.format("pipeline 完成: %d/%d 成功", result.ok, result.ok + result.failed))
```

---

## converge(items, opts?)

对抗性收敛验证。Producer agents 从 items 生成 findings → Adversarial agents 尝试反驳 → 投票决定 surviving findings → 重复直到收敛。

这是 Maestro 独有的功能，Claude Code Dynamic Workflows 无此原语。

### 参数

| 参数 | 类型 | 说明 |
|------|------|------|
| `items` | array | 待验证的 items 列表 |
| `opts` | table | 可选配置 |

`opts` 可选字段：

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `adversarial` | boolean | `true` | 是否启用对抗验证 |
| `vote_threshold` | number | `0.7` | 投票通过阈值（0.0-1.0） |
| `max_rounds` | number | `3` | 最大验证轮次 |
| `producers_per_item` | number | `1` | 每个 item 的 producer agent 数 |
| `adversaries_per_finding` | number | `1` | 每个 finding 的 adversary agent 数 |
| `model` | string | `nil` | agent 使用的模型 |

### 返回值

table，包含：

| 字段 | 类型 | 说明 |
|------|------|------|
| `converged` | boolean | 是否收敛 |
| `rounds` | number | 实际执行轮次 |
| `findings` | array | 最终 surviving findings |
| `round_stats` | array | 每轮的统计（items/findings/surviving） |

### 示例

```lua
local claims = {
    "API 端点 /users 需要 RBAC 鉴权",
    "密码存储使用了 bcrypt 哈希",
    "输入验证覆盖了 SQL 注入"
}
local result = converge(claims, {
    adversarial = true,
    vote_threshold = 0.7,
    max_rounds = 3
})
if result.converged then
    log("收敛完成，共 " .. #result.findings .. " 条 findings")
end
```

---

## workflow(path, args?)

嵌套子工作流。加载并执行另一个 Lua 脚本，共享全局并发 cap。

### 参数

| 参数 | 类型 | 说明 |
|------|------|------|
| `path` | string | 子工作流文件路径 |
| `args` | table | 传递给子工作流的参数 |

### 返回值

子工作流中 `report()` 的值。

### 示例

```lua
local sub_result = workflow("~/workflows/deep-research.lua", {
    topic = "Rust 异步运行时对比"
})
```

---

## phase(name, planned?)

进度分组。将后续的 agent 调用归入同一阶段，用于进度 UI。

### 参数

| 参数 | 类型 | 说明 |
|------|------|------|
| `name` | string | 阶段名称 |
| `planned` | number | 预计 agent 数量（可选） |

### 返回值

`phase_id` (number)

### 示例

```lua
local pid = phase("研究阶段", 5)
-- 后续 agent 调用归属此阶段
```

---

## log(msg, level?)

输出结构化日志。

### 参数

| 参数 | 类型 | 说明 |
|------|------|------|
| `msg` | string | 日志消息 |
| `level` | string | `"info"` / `"warn"` / `"error"`（默认 `"info"`） |

### 示例

```lua
log("开始处理", "info")
log("文件不存在: " .. path, "warn")
```

---

## budget(time_ms?, rounds?)

设置运行时限制提示。

### 参数

| 参数 | 类型 | 说明 |
|------|------|------|
| `time_ms` | number | 时间限制（毫秒） |
| `rounds` | number | 最大轮次限制 |\n
---

## report(value)

设置工作流的最终输出。每个工作流应调用一次。

### 参数

| 参数 | 类型 | 说明 |
|------|------|------|
| `value` | any | 任意 Lua 值（会被序列化为 JSON） |

### 示例

```lua
report({
    status = "complete",
    findings = results,
    summary = "发现 3 个安全问题"
})
```

---

## json.encode(t) / json.decode(s)

序列化辅助。

```lua
local s = json.encode({ key = "value" })  -- '{"key":"value"}'
local t = json.decode(s)                  -- { key = "value" }
```

---

## 沙箱限制

Lua VM 运行在安全沙箱中，以下全局变量被屏蔽：

- `io` — 文件/标准流操作
- `os` — 系统调用（execute, getenv 等）
- 文件系统访问
- 网络访问

三重资源限制：

| 限制 | 说明 |
|------|------|
| 指令计数 | `set_hook` 检测（当前仅检测，未强制终止） |
| 执行时间 | 可通过 `budget()` 设置 |
| 内存 | mlua 内存限制 |
