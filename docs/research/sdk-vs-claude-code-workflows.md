# Maestro Lua SDK vs Claude Code Dynamic Workflows 对比

> 整理日期：2026-06-08  
> 参考来源：[Claude Code 官方文档](https://code.claude.com/docs/en/workflows)、[Maestro SDK 参考](../sdk-reference.md)

---

## 基本信息

| 维度 | Maestro | Claude Code |
|---|---|---|
| 脚本语言 | Lua | JavaScript (ES2022+) |
| 脚本来源 | 用户手写 | Claude 自动生成，也可手写 |
| 运行环境 | Lua VM 沙箱（mlua） | 独立 JS 运行时 |
| 最大并发 agents | 未明确文档化 | 16 |
| 总 agent 上限/run | 未明确文档化 | 1,000 |
| Resume / 缓存 | blake3(prompt + model + phase_id) | 同 session 内可恢复 |

---

## 原语总览

| 原语 | Maestro | Claude Code | 备注 |
|---|---|---|---|
| `agent()` | ✅ | ✅ | 签名与返回值有差异 |
| `parallel()` | ✅ | ✅ | API 形态不同 |
| `pipeline()` | ✅ | ✅ | 返回值结构不同 |
| `converge()` | ✅ | ❌ | Maestro 独有 |
| `workflow()` | ✅ | ✅ | 引用方式不同 |
| `phase()` | ✅ | ✅ | Maestro 多 `planned` 参数 |
| `log()` | ✅ | ✅ | Maestro 多 `level` 参数 |
| `budget()` | ✅ 函数（设置限制） | ✅ 只读对象（查询 token） | 语义完全不同 |
| `report()` | ✅ | ❌ | Maestro 独有，用 `return` 替代 |
| JSON 工具 | `json.encode/decode` | 原生 `JSON.stringify/parse` | — |

---

## 逐原语对比

### `agent()`

| 对比项 | Maestro | Claude Code |
|---|---|---|
| 签名 | `agent({prompt, model, schema})` | `agent(prompt, {label, phase, schema, model, isolation, agentType})` |
| 返回值 | `{status, output, tokens, findings}` | 文本字符串，或 schema 约束下的对象 |
| `status` 字段 | ✅ `"success"/"error"` | ❌ 靠异常或 null 判断 |
| `tokens` 字段 | ✅ | ❌ |
| `findings` 字段 | ✅ | ❌ |
| worktree 隔离 | ❌ | ✅ `isolation: "worktree"` |
| 自定义 agent 类型 | ❌ | ✅ `agentType: "Explore"` 等 |

Maestro 返回结构更丰富，便于在脚本内做细粒度的状态判断和 token 统计；Claude Code 更简洁，但增加了 worktree 隔离和自定义 agent 类型两个功能。

---

### `parallel()`

| 对比项 | Maestro | Claude Code |
|---|---|---|
| 签名 | `parallel(items, mapFn)` | `parallel(thunks[])` |
| 调用形态 | items 数组 + 映射函数 | 零参函数（thunk）数组 |
| 失败行为 | 未明确 | 失败项返回 `null`，整体不抛异常 |
| 语义 | 栅栏同步（barrier） | 栅栏同步（barrier） |

```lua
-- Maestro
parallel(files, function(f) return {prompt = "审查 " .. f} end)
```

```js
// Claude Code
parallel(files.map(f => () => agent(`审查 ${f}`)))
```

Claude Code 的 thunk 设计更灵活，可在 thunk 内写任意逻辑；Maestro 的 `mapFn` 写法更简洁。

---

### `pipeline()`

| 对比项 | Maestro | Claude Code |
|---|---|---|
| 签名 | `pipeline(items, stages[])` | `pipeline(items, stage1, stage2, ...)` |
| 阶段定义 | `{name, handler}` table | 裸函数，可变参数 |
| handler 签名 | `function(item, prev_result)` | `(prevResult, originalItem, index)` |
| 返回值 | `{ok, failed, total_stages, total_elapsed_ms, items}` | `any[]` 每 item 最终结果 |
| 阶段命名 | ✅ 必须有 `name` 字段 | ❌ 无，靠 `opts.phase` 分组 |

Maestro 返回聚合统计（ok/failed/elapsed），监控友好；Claude Code 的 handler 额外提供 `originalItem` 和 `index`，后置阶段引用原始数据时更方便。

---

### `converge()` — Maestro 独有

Claude Code **没有**此原语。Maestro 内置了完整的对抗性收敛验证循环：

```
producers → findings → adversaries 投票 → surviving findings → 下一轮
```

在 Claude Code 中要实现等效效果，需要手写 `while` + `parallel()` 组合，且无内置的投票和收敛判断逻辑。

**opts 参数：**

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `adversarial` | boolean | `true` | 是否启用对抗验证 |
| `vote_threshold` | number | `0.7` | 投票通过阈值（0.0–1.0） |
| `max_rounds` | number | `3` | 最大轮次 |
| `producers_per_item` | number | `1` | 每 item 的 producer 数 |
| `adversaries_per_finding` | number | `1` | 每 finding 的 adversary 数 |
| `model` | string | nil | 使用的模型 |

**返回值：** `{converged, rounds, findings, round_stats}`

---

### `workflow()`

| 对比项 | Maestro | Claude Code |
|---|---|---|
| 签名 | `workflow(path, args?)` | `workflow(nameOrRef, args?)` |
| 引用方式 | 文件路径（`~/workflows/x.lua`） | 名称字符串或 `{scriptPath: "..."}` |
| 嵌套限制 | 未明确 | 最多 1 层 |
| 共享资源 | 共享全局并发 cap | 共享并发 cap、agent 计数、token budget |
| 返回值 | 子工作流 `report()` 的值 | 子工作流 `return` 的值 |

---

### `phase()`

| 对比项 | Maestro | Claude Code |
|---|---|---|
| 签名 | `phase(name, planned?)` | `phase(title)` |
| `planned` 参数 | ✅ 预报 agent 数，UI 显示 "2/5" 进度 | ❌ 无 |
| 返回值 | `phase_id` (number) | 无 |
| parallel/pipeline 内 | 隐式继承当前 phase | 推荐显式传 `opts.phase`，避免竞态 |

---

### `log()`

| 对比项 | Maestro | Claude Code |
|---|---|---|
| 签名 | `log(msg, level?)` | `log(msg)` |
| level 支持 | ✅ `"info"/"warn"/"error"` | ❌ 无 |

---

### `budget()`

两者语义**完全不同**：

| 对比项 | Maestro | Claude Code |
|---|---|---|
| 类型 | 函数，**设置**时间/轮次上限 | 只读对象，**查询** token 用量 |
| 签名 | `budget(time_ms?, rounds?)` | `budget.total` / `budget.spent()` / `budget.remaining()` |
| 控制粒度 | 时间（ms）+ 轮次 | Token 用量 |
| 典型用途 | 防止 converge 无限运行 | 动态调整 agent 规模 |

```lua
-- Maestro：限制最多 2 分钟或 5 轮
budget(120000, 5)
```

```js
// Claude Code：按剩余 token 动态扩缩
while (budget.total && budget.remaining() > 50_000) {
    const result = await agent("继续分析...")
}
```

---

### `report()` — Maestro 独有

Maestro 用 `report(value)` 显式设置工作流的最终输出，整个工作流只调用一次。

Claude Code 没有此函数，等效方式是在脚本末尾使用 `return` 语句，或依赖最后一个 agent 的文本输出作为结果。

---

## 沙箱限制对比

| 限制类型 | Maestro | Claude Code |
|---|---|---|
| 文件系统 | ❌ `io` 被屏蔽 | ❌ 脚本本身不可访问，由 agent 代操作 |
| 系统调用 | ❌ `os` 被屏蔽 | ❌ 同上 |
| 时间/随机 | 未明确屏蔽 | ❌ `Date.now()`/`Math.random()`/`new Date()` 抛异常（破坏 resume 缓存） |
| 网络 | ❌ 被屏蔽 | ❌ 由 agent 代操作 |
| 指令计数 | ✅ 检测（未强制终止） | — |
| 内存 | ✅ mlua 限制 | — |

---

## 功能差异汇总

| 能力 | Maestro | Claude Code |
|---|---|---|
| 对抗性收敛（`converge`） | ✅ 内置原语 | ❌ 需手写 |
| agent 返回 status/tokens/findings | ✅ | ❌ |
| phase 进度百分比（`planned`） | ✅ | ❌ |
| log 分级（warn/error） | ✅ | ❌ |
| budget 时间/轮次限制 | ✅ | ❌ |
| `report()` 显式输出 | ✅ | ❌ |
| worktree 隔离执行 | ❌ | ✅ |
| 自定义 agentType | ❌ | ✅ |
| token-aware 动态扩缩 | ❌ | ✅ |
| pipeline handler 拿到 originalItem/index | ❌ | ✅ |
| 脚本语言表达力 | Lua（轻量，沙箱友好） | JS（表达力强，生态广） |

**Maestro 的优势**：监控粒度更细（tokens/status/planned/log level）、内置 `converge` 原语省去手写对抗验证循环、`budget` 直接限制时间和轮次。

**Claude Code 的优势**：worktree 隔离保证并行写文件无冲突、agentType 复用专用 agent、token-aware 动态决策工作量、JS 的表达能力更适合复杂控制流。
