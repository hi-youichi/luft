# MCP workflow 工具 — 向 loom 对齐

> **状态**: §3.1-§3.5 + §4 已实现并通过测试。§3.6（`get_run_source`）按 §6 问题 4 的决定推迟，未实现。
> **目标**: 把 `luft-mcp`（`luft mcp serve`）暴露的 workflow 管理工具，对齐到 loom `tool-workflow` crate 里 `workflow_*` 工具集的能力面（参数、返回结构、分页/过滤、resume/cancel）。

---

## 1. 背景

loom 的 `agent/tool/tool-workflow` crate **直接依赖 `luft`/`luft-core`（crates.io `luft = "0.3"`）作为执行引擎**——它不是"另一套类似实现"，而是在同一个 `luft::Luft`/`RunHandle` API 上包了一层更完整的工具面。对照下来，`luft-mcp` 自己暴露给外部 MCP client 的 4 个工具明显更薄：

| 能力 | loom（`tool-workflow/src/*.rs`） | luft-mcp（`crates/luft-mcp/src/*.rs`，当前实现） |
|------|------|------|
| 启动 workflow | `workflow_start`：script / workflow 文件 / **resume_from_id** 三选一 + `concurrency` | `execute_workflow`：script / path 二选一，无 resume、无并发参数 |
| 查询状态 | `workflow_status`：富摘要（agents/phase_spans/event_stats/report）+ 脱敏 | `get_run_status`：`StatusOutput` 原样序列化，扁平 |
| 查询事件 | `workflow_events`：`offset`/`events_limit`(≤500)/`types[]`/`agent_id` 过滤，真分页 | `get_run_events`：仅 `since_event_id` 单游标 |
| 列出 run 历史 | `workflow_list`：`limit`/`cursor`/`status_filter` 分页 | ❌ 无 |
| 列出 .lua 文件 | `workflow_files` | `list_workflows` |
| 取消在跑的 run | `workflow_cancel` | ❌ 无 |
| 读回执行过的源码 | `workflow_source` | ❌ 无 |

关键事实：**loom 能做到 resume/cancel，不是因为它重新实现了引擎能力，而是因为 `luft::Luft` 本身已经有 `start_resume()` / `cancel()` / `RunHandle::cancel()`（见 [`builder.rs:244`](../../crates/luft/src/builder.rs)、[`builder.rs:307`](../../crates/luft/src/builder.rs)、[`builder.rs:361`](../../crates/luft/src/builder.rs)）**。loom 只是把已有的引擎能力接到了工具层；`luft-mcp` 至今没有对外暴露这两个能力。`workflow_source`（读回执行过的 Lua 源码）是唯一一个 loom 自己额外持久化实现的能力，luft 引擎本身不落这份文件，对齐这一项需要新增落盘逻辑。

---

## 2. 对齐范围与不对齐的部分

**对齐**（复用 loom 已验证过的参数/返回设计）：
- resume 语义、cancel 语义、事件分页/过滤、run 历史列表分页、状态返回的脱敏思路
- 标识符收敛为单一字符串（见 §4，这是最大的破坏性变更）

**不对齐**（保留 luft 自己的选择，原因见括号）：
- **工具命名风格**：本文沿用现有真实工具名（`execute_workflow`/`list_workflows`/`get_run_status`/`get_run_events`，见 [`docs/mcp-server.md`](../mcp-server.md)），不改成 loom 的 `workflow_start` 风格；`docs/tool-reference.md`、[`docs/design/tool-registry.md`](./tool-registry.md) 两份草案用的是另一套尚未实现的点号命名（`workflow.execute` 等），跟当前真实实现和本文档都不一致，等 tool-registry.md 那套 Registry 重构真正推进时再统一。
- **执行引擎实现**：不打算像 loom 一样把 luft 当外部 crate 依赖拉进来（luft 本来就是自己），这里说的"对齐"仅指 `luft-mcp` 这一层工具 API 的形状。

---

## 3. 逐工具目标设计

### 3.1 `execute_workflow`（现有，扩展）

**新增字段**（对齐 `workflow_start`，见 [`tool_start.rs:65-104`](../../../loom/agent/tool/tool-workflow/src/tool_start.rs)）：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `script` | string | 三选一 | 内联 Lua（不变） |
| `path` | string | 三选一 | `.lua` 文件路径（不变，loom 叫 `workflow`，沿用 luft 现有命名） |
| `resume_from_id` | string | 三选一（**新增**） | 之前一次 run 的标识符；从 checkpoint 恢复，跳过已完成 phase。与 `script`/`path` 互斥 |
| `args` | object | 否 | 不变 |
| `concurrency` | integer, 1–64, 默认 4 | 否（**新增**） | 直接透传给 `LuftBuilder::concurrency()`（已存在，见 loom `service.rs:114`） |

**返回**：
```json
{ "run_id": "...", "status": "running", "resumed_from": "..." }
```
`resumed_from` 仅在走 resume 分支时出现（对齐 loom `service.rs:181-183`）。

**实现落点（已完成）**：`crates/luft-mcp/src/tools.rs::execute_workflow` 新增了 resume 分支（调 `luft.start_resume(id)`）和 `concurrency` 支持。`concurrency` 的解法没有改 `McpServer` 的生命周期（它仍然只在启动时构造一次 `Luft`）——而是在 [`luft/src/builder.rs`](../../crates/luft/src/builder.rs) 给 `Luft` 加了一个新方法 `with_concurrency(&self, n) -> Luft`：clone 同一个 backend/base_dir/planner_config/exec_limits，只换 concurrency 值，返回一个独立可用的新 `Luft`。`execute_workflow` 收到 `concurrency` 参数时用这个新实例发起 run，不带就用共享的 `self.luft`——不需要 `McpServer` 存原始 backend/base_dir 就能支持 per-call concurrency（回答了 §6 问题 3）。

### 3.2 `get_run_status`（现有，扩展）

参数不变（`run_id` 必填,沿用现有命名,不改成 loom 的 `instance`）。

**返回扩展（已实现）**：`crates/luft-mcp/src/tools.rs::build_rich_status` 在 `StatusOutput` 之上叠加了 `total_phases`/`phases[]`/`report`/`error`。`phases[]`/`agents[]` 是从 `luft.events(run_id)` 扫一遍事件日志派生出来的（`derive_phases`）——`StatusOutput` 本身只有聚合计数，没有逐 phase/agent 明细，事件日志是唯一的数据源。`report` 来自 `luft.report(run_id)`；`error` 是尽力而为：`AgentEvent::RunDone` 本身没有错误信息字段，只有当 `status=="failed"` 时才去倒序扫 `Log{level:Error}` 事件取最后一条当错误信息，没有就是 `null`。

**脱敏**：这次没有实现——loom 那套"剥离 `workflow.path`/`output_ref`/`checkpoint_hash`"是因为它的底层数据里真的带着这些内部路径字段；luft 现在的 `StatusOutput`/事件日志里没有这类字段，脱敏没有实际对象可剥，暂不需要。以后 `StatusOutput` 如果加了内部路径/哈希类字段，要重新考虑这条。

### 3.3 `get_run_events`（现有，扩展）

**参数扩展**（对齐 `workflow_events`，见 [`tool_events.rs:37-68`](../../../loom/agent/tool/tool-workflow/src/tool_events.rs)）：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `run_id` | string | ✅ | 不变 |
| `offset` | integer, 默认 0 | 否（**新增**） | 跳过前 N 条匹配事件 |
| `events_limit` | integer, 1–500, 默认 50 | 否（**新增**） | 分页大小 |
| `types` | string[] \| null | 否（**新增**） | 按事件 `type` 过滤 |
| `agent_id` | string \| null | 否（**新增**） | 按 agent 过滤 |
| `since_event_id` | string | 否（**保留**） | 现有的增量轮询游标；与 `offset` 是两种独立分页方式，都保留——`since_event_id` 面向"我上次看到哪"，`offset`/`events_limit` 面向"我要第几页" |

**返回扩展**：
```json
{
  "events": [...],
  "offset": 0,
  "events_limit": 50,
  "total_matching": 137,
  "next_offset": 50
}
```
`next_offset` 为 `null` 表示已到最后一页（对齐 `service.rs:736-740`）。**已实现**——`since_event_id` 子串匹配先应用，再叠加 `types`/`agent_id` 过滤和 `offset`/`events_limit` 分页。

### 3.4 `list_workflows` — 拆成两个工具（破坏性变更）

**当前 `list_workflows` 实际列的是 `.lua` 文件**（对应 loom 的 `workflow_files`，不是 loom 的 `workflow_list`）。这个命名本身有歧义,对齐时必须先拆开,否则加了"列历史 run"的分页参数会让人以为在给"列文件"分页:

- **`list_files`**（=现有 `list_workflows` 改名）：列 `.lua` 文件，参数返回不变
- **`list_runs`**（新增，对齐 `workflow_list`/`list_instances`，见 [`tool_list.rs`](../../../loom/agent/tool/tool-workflow/src/tool_list.rs)、[`service.rs:459-537`](../../../loom/agent/tool/tool-workflow/src/service.rs)）：

  | 字段 | 类型 | 必填 | 说明 |
  |------|------|------|------|
  | `limit` | integer, 1–100, 默认 20 | 否 | |
  | `cursor` | string | 否 | 上一页 `next_cursor` |
  | `status_filter` | enum: completed\|failed\|cancelled | 否 | 大小写不敏感 |

  返回：`{ "runs": [...], "count": N, "next_cursor": "...", "has_more": bool }`（字段名 `runs` 而非 loom 的 `instances`，跟 luft 其余工具的 `run_id` 措辞保持一致）

**已实现，按 §6 问题 1 的决定拆分**——`list_workflows` 直接改名成 `list_files`，`list_runs` 是新工具。这是对外 API 的 breaking change（如果已经有外部 client 用旧名字会断），`docs/mcp-server.md` 已经同步改过。

### 3.5 `cancel_run`（新增）

对齐 [`tool_cancel.rs`](../../../loom/agent/tool/tool-workflow/src/tool_cancel.rs)：

| 字段 | 类型 | 必填 |
|------|------|------|
| `run_id` | string | ✅ |

返回：
```json
{ "run_id": "...", "result": "cancelling" }
```
或
```json
{ "run_id": "...", "result": "not_found_or_terminal", "note": "..." }
```

**实现落点（已完成）**：`cancel_run_tool` 先调 `luft.status(run_id)` 判断是否已是终态（completed/failed/cancelled）或压根不存在，两种都返回 `not_found_or_terminal`；否则调 `luft.cancel(&run_dir)`（已存在，[`builder.rs:307`](../../crates/luft/src/builder.rs)）返回 `cancelling`。比 loom 简单——loom 还要维护一个跨 agent 的 `CancellationToken` 注册表因为它允许"别的 agent 取消这次 run"；`luft-mcp` 目前是单连接单进程模型，没有做这层注册表，直接透传到 `Luft::cancel`。

### 3.6 `get_run_source`（新增，需要新增落盘）

对齐 [`tool_source... (workflow_source in service.rs:810-846)`](../../../loom/agent/tool/tool-workflow/src/service.rs)。

**前置缺口**：luft 引擎本身不把执行过的 Lua 源码存进 run 目录（确认过 `crates/luft/src/builder.rs` 和 `crates/luft-runtime` 都没有写 `workflow.lua` 这类文件）。loom 是自己的 `WorkflowRuntime` 额外做的。要在 luft 侧提供这个工具，`Luft::start_script`/`start_resume` 需要在 run 目录里落一份源码副本——**这是唯一一项需要动 luft 核心引擎（而不只是 luft-mcp 工具层）的对齐项**，工作量比其他几项大,建议放到单独一个里程碑。

| 字段 | 类型 | 必填 |
|------|------|------|
| `run_id` | string | ✅ |

返回：`{ "run_id": "...", "source": "...", "truncated": bool }`（预览上限对齐 loom 的 8192 字节,超出截断加省略号）

---

## 4. 标识符收敛（最大的破坏性变更）

现状：`luft-mcp` 对外用 `run_id`（UUID，`Luft::start_script` 生成），内部再通过 `RunRegistry`（[`tools.rs:33`](../../crates/luft-mcp/src/tools.rs)）映射到 `run_dir_name`；两层indirection 只是为了兼容"重启 luft-mcp 进程后 RunRegistry 清空、但历史 run 目录还在磁盘上"这种情况（`resolve_run_dir` 兜底把输入当 run_dir 本身用,见 [`tools.rs:172-177`](../../crates/luft-mcp/src/tools.rs)）。

loom 完全不做这层区分——`instance_dir` 本身就是磁盘目录名,同时也是外部标识符,没有单独的 UUID。

**已实现**：字段名仍是 `run_id`（不改成 loom 的 `instance`/`instance_dir`,原因见 §2）,值直接等于 run 目录名。`RunRegistry`/`RunInfo`/`new_run_registry` 整个删掉了,`tools.rs`/`server.rs`/`lib.rs` 相应的签名都去掉了 `runs` 参数。好处是重启 `luft mcp serve` 进程后历史 run 立刻可查,不再依赖内存态的 registry；代价（已接受）：`run_id` 不再是标准 UUID 格式,外部约定依赖这一点的话会破坏——这是一次对外 API breaking change。

---

## 5. 文档同步状态

- [`docs/mcp-server.md`](../mcp-server.md) §3：**已更新**——6 个工具的完整 schema（resume/concurrency/list_files+list_runs/富字段/分页/cancel_run）
- `docs/tool-reference.md`、`docs/design/tool-registry.md`：**未动**——两份都是尚未实现的 Registry 重构草案（点号命名，跟真实实现本来就不一致，见 §2），不属于这次对齐的范围
- `get_run_source`（§3.6）未实现，`docs/mcp-server.md` 里也没有这个工具

---

## 6. 开放问题 — 处理结果

| # | 问题 | 结果 |
|---|------|------|
| 1 | `list_workflows` 改名拆分成 `list_files`/`list_runs`？ | **已实现**（决定：全部实现，见对话记录） |
| 2 | `run_id` 去掉 UUID、直接等于目录名？ | **已实现** |
| 3 | `concurrency` 需要动 `Luft`/`LuftBuilder` API？ | **已实现**——加了 `Luft::with_concurrency()`，是纯新增方法，没有改动 `McpServer` 的生命周期或任何现有签名 |
| 4 | `get_run_source` 单独排期？ | **是——本轮未实现**，需要新增引擎级落盘逻辑（把执行过的 Lua 源码存进 run 目录），工作量和风险都跟前面几项不是一个量级，留到下次单独做 |
| 5 | 工具命名沿用现有真实名称？ | **是**，`execute_workflow`/`list_files`/`list_runs`/`get_run_status`/`get_run_events`/`cancel_run`，没有切到 loom 的 `workflow_start` 风格 |

**本轮之外未做的**：MCP `workflow://reference/{name}` 资源通道（§0/§5 原本就标了不在这轮范围）；脱敏规则（§3.2，暂无实际需要脱敏的字段）。

---

## 7. 相关文档

- 当前实现：[`crates/luft-mcp/src/tools.rs`](../../crates/luft-mcp/src/tools.rs)、[`protocol.rs`](../../crates/luft-mcp/src/protocol.rs)
- loom 参照实现：`agent/tool/tool-workflow/src/{tool_start,tool_status,tool_events,tool_list,tool_cancel,tool_files,service}.rs`（loom 仓库路径，非本仓库）
- 用户指南：[`docs/mcp-server.md`](../mcp-server.md)
- 开发参考：[`docs/tool-reference.md`](../tool-reference.md)
- 工具执行核心重构草案：[`docs/design/tool-registry.md`](./tool-registry.md)
