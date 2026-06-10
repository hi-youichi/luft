# WebSocket 服务器设计

> 为 Maestro 增加 `maestro serve` 子命令，通过 WebSocket 暴露完整的运行控制与事件订阅能力，供 Web UI、IDE 插件、外部自动化等客户端使用。
>
> **设计目标**：基础协议对现有代码侵入最小（仅 `cli::RunArgs` 增加一个可选字段），全部新增能力通过 `src/ws/` 模块和两个 Cargo.toml 依赖实现。
>
> **可选扩展**：若需要 ACP 全保真事件流（agent 消息/reasoning/工具调用原始参数等无损透传），开启 [§3.6 ACP 原始透传](#36-acp-原始透传可选-opt-in)。该扩展会额外触及适配层与事件契约，footprint 大于基础协议，默认关闭。

---

## 目录

1. [背景与目标](#1-背景与目标)
2. [整体架构](#2-整体架构)
3. [消息协议](#3-消息协议)
4. [模块设计](#4-模块设计)
5. [核心流程](#5-核心流程)
6. [CLI 集成](#6-cli-集成)
7. [错误处理](#7-错误处理)
8. [安全设计](#8-安全设计)
9. [依赖增量](#9-依赖增量)
10. [实现计划](#10-实现计划)
11. [已知局限与后续工作](#11-已知局限与后续工作)

---

## 1. 背景与目标

Maestro 目前支持两种输出模式（见 [cli.md §4](../architecture/cli.md)）：

| 模式 | 适用场景 | 局限 |
|------|---------|------|
| **Headless** | 脚本/CI，JSONL stdout | 单次、无交互、无并发控制 |
| **TUI** | 终端交互 | 受限于终端环境 |

WebSocket 服务器补充第三种模式：**面向程序的长连接实时接口**，使客户端能够：

- 提交 run（NL、workflow 文件或直接内嵌 Lua 脚本）并获得 `run_id`
- NL run 在执行前预览生成的 Lua 脚本，客户端确认后再执行
- 订阅任意 run 的 `AgentEvent` 实时流，并可按事件类型过滤
- 恢复（resume）上次未完成的 run
- 取消正在运行的 run
- 查询历史 run 列表、状态快照、事件日志、findings 及最终报告

设计约束：
- 仅在 `cli::RunArgs` 增加一个可选的 `events_tx` 字段（向后兼容，`None` 等价于现有行为）
- 直接复用 `AgentEvent` 广播总线（见 [architecture.md §2](../architecture.md)）
- 复用 `cli::run` 作为执行入口，不重复实现 run 生命周期

---

## 2. 整体架构

```
客户端 (Browser / IDE / 脚本)
        │  WebSocket  ws://127.0.0.1:7474/ws
        ▼
┌──────────────────────────────────────────────────────────────┐
│  src/ws/                                                     │
│                                                              │
│  mod.rs              handler.rs           protocol.rs        │
│  ┌──────────────┐   ┌────────────────┐   ┌───────────────┐  │
│  │  AppState    │   │  handle_ws()   │   │  ClientMsg    │  │
│  │  serve()     │──▶│  读写双循环    │──▶│  ServerMsg    │  │
│  │  axum 路由   │   │  订阅管理      │   │  ErrorCode    │  │
│  └──────┬───────┘   └───────┬────────┘   └───────────────┘  │
│         │                   │                                │
│  registry.rs                │ broadcast::Receiver            │
│  ┌──────────────┐           │                                │
│  │  RunRegistry │◀──────────┘                                │
│  │  RunHandle   │  { events_tx, cancel, task }               │
│  └──────────────┘                                            │
└──────────────────────┬───────────────────────────────────────┘
                       │ Arc<dyn AgentBackend>
                       │ cli::run(backend, RunArgs)
                       ▼
┌──────────────────────────────────────────────────────────────┐
│  现有代码（最小改动）                                         │
│  cli::run → Runtime → Scheduler → AgentBackend               │
│                ↓                                             │
│         broadcast::Sender<AgentEvent>                        │
└──────────────────────────────────────────────────────────────┘
```

**数据流**：`AppState` 持有 `Arc<dyn AgentBackend>` 和一个 `RunRegistry`。每当客户端发送 `run` 消息，服务端调用 `cli::run` 在后台 task 执行，`RunArgs.events_tx` 将外部构造的 `broadcast::Sender<AgentEvent>` 注入 cli，使服务端能订阅同一条事件总线。订阅该 run 的所有 WebSocket 连接从对应的 `broadcast::Receiver` 取事件并转发。

---

## 3. 消息协议

所有消息均为 **UTF-8 JSON 文本帧**，单帧上限 **64 KB**。

每条客户端消息携带必填的 `id` 字段（字符串，客户端自定义），服务端的所有响应消息通过 `req_id` 与之对应。无法关联请求的服务端主动消息（`event`、`hello`、`server_closing`）不含 `req_id` 字段。

---

### 3.1 连接生命周期

```
客户端连接
    │
    ▼
服务端立即发送 hello
    │
    ▼
正常通信（客户端发请求，服务端发响应 + 主动推送）
    │
    ├── 客户端断开 → 连接清理（取消所有该连接持有的订阅）
    │
    └── 服务端关闭（Ctrl-C）→ 服务端发 server_closing → 关闭连接
```

WebSocket 层面的心跳：客户端发 `ping`，服务端回 `pong`。服务端也会主动发 WebSocket 协议级 Ping 帧（axum 默认行为），客户端应回 Pong 帧（浏览器自动处理）。

---

### 3.2 客户端 → 服务端（`ClientMsg`）

#### `run` — 提交新 run

`nl`、`workflow`、`script` 三选一：

```json
{
  "type": "run",
  "id": "req-1",
  "payload": {
    "nl":       "分析这段代码并找出性能瓶颈",
    "workflow": null,
    "script":   null,
    "args":     { "focus": "hot path" },
    "confirm":  false
  }
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `nl` | `string \| null` | 自然语言提示，由 planner 生成 Lua 脚本后执行 |
| `workflow` | `string \| null` | workflow 文件的**绝对路径**（服务端本地文件系统） |
| `script` | `string \| null` | 直接内嵌的 Lua 脚本字符串 |
| `args` | `object` | 传给脚本的参数，默认 `{}` |
| `confirm` | `bool` | `true` 时 NL run 在执行前先返回 `script_preview`，等待客户端发 `confirm_run` 后再执行；默认 `false` |

`nl` + `confirm: true` 的交互流程见 [§5.2](#52-nl-run-脚本预览与确认)。

#### `confirm_run` — 确认执行预览中的脚本

```json
{
  "type": "confirm_run",
  "id":   "req-2",
  "payload": {
    "run_id":  "019xxx-...",
    "approve": true
  }
}
```

`approve: false` 时服务端放弃该 run，清理 RunRegistry 并归还并发 permit。

#### `resume` — 恢复上次未完成的 run

```json
{
  "type": "resume",
  "id":   "req-3",
  "payload": {
    "run_id": "019xxx-..."
  }
}
```

服务端从 `.maestro/runs/<run_id>/` 恢复执行，行为等价于 `maestro run --resume`。响应为 `accepted`（成功）或 `error`（run 不存在、已终态、无 workflow.lua）。

#### `cancel` — 取消运行中的 run

```json
{
  "type": "cancel",
  "id":   "req-4",
  "payload": { "run_id": "019xxx-..." }
}
```

`ok` 响应仅表示取消信号已发出（异步），run 真正终止时所有订阅者会收到 `event { run_done { status: "cancelled" } }`。

#### `subscribe` — 订阅 run 的实时事件

```json
{
  "type": "subscribe",
  "id":   "req-5",
  "payload": {
    "run_id":  "019xxx-...",
    "filter":  ["agent_started", "agent_done", "run_done", "log"]
  }
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `run_id` | `string` | 目标 run |
| `filter` | `string[] \| null` | 只推送列出的事件类型；`null` 或省略表示推送全部**投影事件**（不含 `acp_raw`，见下） |

订阅成功后服务端开始推送 `event` 消息，直到 run 结束或客户端发 `unsubscribe`。订阅**已完成**的 run 返回 `error(run_finished)`（区别于 run 从未存在的 `not_found`）。

> **`acp_raw` 必须显式订阅**：高频的 ACP 原始事件（[§3.6](#36-acp-原始透传可选-opt-in)）即使 `filter: null` 也**不会**默认下发，必须在 `filter` 中显式列入 `"acp_raw"`，避免普通客户端被 firehose 淹没。前提是服务端以 `--acp-raw` 启动；否则该事件根本不产生。

#### `unsubscribe` — 取消订阅

```json
{
  "type": "unsubscribe",
  "id":   "req-6",
  "payload": { "run_id": "019xxx-..." }
}
```

#### `get_status` — 查询单个 run 的状态快照

```json
{
  "type": "get_status",
  "id":   "req-7",
  "payload": { "run_id": "019xxx-..." }
}
```

一次性查询，不建立持续订阅。

#### `list_runs` — 列出历史 run

```json
{
  "type": "list_runs",
  "id":   "req-8",
  "payload": {
    "limit":  20,
    "offset": 0
  }
}
```

#### `get_logs` — 获取 run 的历史事件日志

```json
{
  "type": "get_logs",
  "id":   "req-9",
  "payload": {
    "run_id": "019xxx-...",
    "limit":  100,
    "offset": 0
  }
}
```

从 `events.jsonl` 读取，用于补查已完成 run 的完整事件序列。

#### `get_findings` — 获取 run 的 findings

```json
{
  "type": "get_findings",
  "id":   "req-10",
  "payload": { "run_id": "019xxx-..." }
}
```

返回该 run 所有 `Finding`（结构见 [`src/core/contract/finding.rs`](../../src/core/contract/finding.rs)）。

#### `get_report` — 获取 run 的最终报告

```json
{
  "type": "get_report",
  "id":   "req-11",
  "payload": { "run_id": "019xxx-..." }
}
```

从 `events.jsonl` 中**扫描最后一条 `run_done` 事件**取其 `report` 字段，用于在错过实时事件后补取报告。

> 注意：`RunCheckpoint`（`state.rs`）**不持久化 report**，其字段仅含 `run_id/task/status/current_phase/completed_phases/agent_results/findings/total_tokens/created_at/updated_at`。report 只存在于事件日志的 `run_done` 事件中，因此 `get_report` 必须读 `events.jsonl` 而非 checkpoint。

#### `ping` — 连接保活

```json
{ "type": "ping", "id": "req-12" }
```

---

### 3.3 服务端 → 客户端（`ServerMsg`）

#### `hello` — 连接建立后立即发送

```json
{
  "type":    "hello",
  "version": "0.1.0",
  "server":  "maestro",
  "capabilities": ["run", "confirm_run", "resume", "cancel",
                   "subscribe", "unsubscribe", "get_status", "list_runs",
                   "get_logs", "get_findings", "get_report",
                   "script_preview", "event_filter"]
}
```

客户端据此判断服务端支持的消息类型，不需要硬编码版本判断。

#### `accepted` — run 已创建（回应 `run` / `resume`）

```json
{
  "type":   "accepted",
  "req_id": "req-1",
  "run_id": "019xxx-..."
}
```

#### `script_preview` — NL 生成脚本后等待确认（`confirm: true` 时）

```json
{
  "type":   "script_preview",
  "req_id": "req-1",
  "run_id": "019xxx-...",
  "script": "-- 由 planner 生成\nlocal result = agent({ prompt = '...' })\n..."
}
```

客户端收到后应向用户展示脚本，再发 `confirm_run`。若 30 秒内未收到 `confirm_run`，服务端自动放弃并回复 `error(confirm_timeout)`。

#### `event` — 实时事件（订阅后持续推送）

```json
{
  "type":   "event",
  "run_id": "019xxx-...",
  "event":  {
    "type":           "agent_started",
    "run_id":         "019xxx-...",
    "phase_id":       0,
    "agent_id":       "...",
    "prompt_preview": "分析 src/main.rs...",
    "model":          "claude-opus-4-5"
  }
}
```

`event` 字段直接复用 `AgentEvent` 的 serde 序列化（`#[serde(tag = "type", rename_all = "snake_case")]`）。字段结构见源码 [`src/core/contract/event.rs`](../../src/core/contract/event.rs)。

#### `status` — 状态快照（回应 `get_status`）

```json
{
  "type":   "status",
  "req_id": "req-7",
  "run_id": "019xxx-...",
  "data": {
    "run_id":           "019xxx-...",
    "task":             "分析这段代码",
    "status":           "running",
    "current_phase":    1,
    "completed_phases": 1,
    "total_agents":     4,
    "completed_agents": 2,
    "total_tokens":     12400,
    "created_at":       "2025-01-01T00:00:00Z",
    "updated_at":       "2025-01-01T00:00:05Z"
  }
}
```

`data` 字段与 `cli::StatusOutput` 结构一致。

#### `run_list` — 历史 run 列表（回应 `list_runs`）

```json
{
  "type":   "run_list",
  "req_id": "req-8",
  "total":  42,
  "items": [
    {
      "run_id":      "019xxx-...",
      "task":        "分析这段代码",
      "status":      "completed",
      "total_tokens": 12400,
      "updated_at":  "2025-01-01T00:00:05Z"
    }
  ]
}
```

#### `logs` — 历史事件日志（回应 `get_logs`）

```json
{
  "type":   "logs",
  "req_id": "req-9",
  "run_id": "019xxx-...",
  "total":  237,
  "items":  [
    { "type": "run_started", "run_id": "...", "task": "...", "ts": "..." },
    { "type": "agent_started", "..." : "..." }
  ]
}
```

`items` 为 `AgentEvent` 数组，结构与实时 `event.event` 字段一致，保证客户端只需一套解析逻辑。

#### `findings` — findings 列表（回应 `get_findings`）

```json
{
  "type":   "findings",
  "req_id": "req-10",
  "run_id": "019xxx-...",
  "items": [
    {
      "kind":     "missing_auth",
      "severity": "high",
      "title":    "API 端点缺少认证",
      "detail":   "src/api.rs:42 的 /admin 路由未验证 token",
      "location": { "file": "src/api.rs", "line": 42 },
      "evidence": ["GET /admin 可在未登录状态下访问"],
      "data":     {}
    }
  ]
}
```

#### `report` — 最终报告（回应 `get_report`）

```json
{
  "type":   "report",
  "req_id": "req-11",
  "run_id": "019xxx-...",
  "data":   { "markdown": "# 分析报告\n..." }
}
```

`data` 与 `RunDone.report` 字段一致（任意 JSON，约定同 `cli::write_report`）。

#### `ok` — 操作成功（cancel / unsubscribe / confirm_run(approve:false) 等）

```json
{ "type": "ok", "req_id": "req-4" }
```

#### `error` — 操作失败

```json
{
  "type":    "error",
  "req_id":  "req-4",
  "code":    "not_found",
  "message": "run 019xxx-... not found"
}
```

#### `server_closing` — 服务端即将关闭

```json
{
  "type":   "server_closing",
  "reason": "shutdown"
}
```

服务端收到 Ctrl-C 后广播此消息，随后关闭所有连接。客户端收到后应停止重连。

#### `pong`

```json
{ "type": "pong", "req_id": "req-12" }
```

---

### 3.4 事件类型速查

以下 `AgentEvent` 变体会被 `event` 消息和 `logs.items` 复用（字段详见源码）：

| `event.type` | 含义 | 高频？ |
|-------------|------|-------|
| `run_started` | run 启动，含 task 描述 | — |
| `phase_started` | 新阶段开始，含 label 和计划 agent 数 | — |
| `agent_started` | 单个 agent 开始执行，含 prompt 预览 | — |
| `agent_progress` | agent 执行中进度（消息/工具调用/文件编辑/token） | ⚠️ 高频 |
| `agent_done` | agent 完成，含状态/token/耗时 | — |
| `phase_done` | 阶段结束，含 ok/failed 计数 | — |
| `run_done` | run 结束，含最终状态和 report | — |
| `log` | 运行时日志，含 level 和 msg | — |
| `pipeline_started` | pipeline 启动 | — |
| `pipeline_stage_started` | pipeline 单阶段开始 | — |
| `pipeline_item_done` | pipeline 单 item 完成 | — |
| `pipeline_done` | pipeline 全部完成 | — |
| `acp_raw` | ACP 原始 SessionUpdate 透传（仅 `--acp-raw` 启动 + 显式订阅） | ⚠️⚠️ 极高频 |

> `agent_progress` 在活跃 run 中非常高频（每个 token delta 一条）。只关心里程碑事件的客户端应在 `subscribe` 时传 `filter`，排除 `agent_progress` 以减少流量。
>
> `agent_progress` 是 ACP 的**有损投影**（仅 message/tool_call/file_edit/tokens 四类，reasoning 被并入 message 加 `[reasoning]` 前缀）。需要 ACP 全保真的客户端改订阅 `acp_raw`，见 [§3.6](#36-acp-原始透传可选-opt-in)。

---

### 3.5 完整消息类型速查

| 方向 | `type` | 触发 / 含义 |
|------|--------|-------------|
| S→C | `hello` | 连接建立后主动发送，含版本与能力列表 |
| C→S | `run` | 提交新 run（NL / workflow 路径 / script 内嵌） |
| C→S | `confirm_run` | 确认或拒绝 `script_preview` |
| C→S | `resume` | 恢复未完成的 run |
| C→S | `cancel` | 发出取消信号 |
| C→S | `subscribe` | 订阅实时事件，支持类型过滤 |
| C→S | `unsubscribe` | 取消订阅 |
| C→S | `get_status` | 查询状态快照 |
| C→S | `list_runs` | 列出历史 run（分页） |
| C→S | `get_logs` | 获取历史事件日志（分页） |
| C→S | `get_findings` | 获取 findings |
| C→S | `get_report` | 获取最终报告 |
| C→S | `ping` | 保活 |
| S→C | `accepted` | run 已创建，含 run_id |
| S→C | `script_preview` | NL 生成脚本预览，等待确认 |
| S→C | `event` | 实时 AgentEvent（订阅后推送） |
| S→C | `status` | 状态快照（回应 get_status） |
| S→C | `run_list` | 历史 run 列表（回应 list_runs） |
| S→C | `logs` | 历史事件（回应 get_logs） |
| S→C | `findings` | findings 列表（回应 get_findings） |
| S→C | `report` | 最终报告（回应 get_report） |
| S→C | `ok` | 操作成功（cancel / unsubscribe 等） |
| S→C | `error` | 操作失败，含错误码 |
| S→C | `server_closing` | 服务端即将关闭 |
| S→C | `pong` | 回应 ping |

---

### 3.6 ACP 原始透传（可选，opt-in）

默认的 `agent_progress` 是 ACP `SessionUpdate` 的**有损投影**——只保留 message/tool_call/file_edit/tokens 四类，且丢弃 tool call id、原始 input 参数、工具结果、`Plan` 更新、多 content block 等。需要**全保真**重建 agent 行为（例如复刻思考过程的 Web UI）的客户端，可订阅原始 ACP 流。

#### 启用条件

| 层级 | 要求 |
|------|------|
| 服务端 | 以 `maestro serve --acp-raw` 启动；不带该标志时 `acp_raw` 事件**根本不产生**（零成本） |
| 客户端 | `subscribe` 的 `filter` 中显式列入 `"acp_raw"`；`filter: null` 不会下发 |
| 握手 | 服务端开启时，`hello.capabilities` 包含 `"acp_raw"`，客户端据此探测 |

#### `acp_raw` 事件结构

```json
{
  "type":   "event",
  "run_id": "019xxx-...",
  "event": {
    "type":     "acp_raw",
    "run_id":   "019xxx-...",
    "agent_id": "...",
    "seq":      42,
    "update": {
      "sessionUpdate": "agent_message_chunk",
      "content": [{ "type": "text", "text": "..." }]
    }
  }
}
```

| 字段 | 说明 |
|------|------|
| `agent_id` | 产生该 update 的 agent |
| `seq` | **per-agent 单调递增**序号；客户端据此检测丢帧/乱序，是无损重建的关键 |
| `update` | ACP `SessionUpdate` 的**原样 serde 序列化**（tag 字段为 `sessionUpdate`），不做任何投影/裁剪 |

`update` 涵盖全部 ACP 变体：`agent_message_chunk` / `agent_thought_chunk`（reasoning，无前缀污染）/ `tool_call`（含 id、input、locations）/ `tool_call_update`（状态流转）/ `plan` / `usage_update` / `user_message_chunk` 等。

#### 与投影事件的关系

`acp_raw` 与 `agent_progress` **并行发出、互不替代**：

- TUI / 普通客户端继续消费 `agent_progress`（轻量、稳定）
- 全保真客户端订阅 `acp_raw`，可完全忽略 `agent_progress`
- 两者可同时订阅（如需对照），靠 `seq` 与 `agent_id` 关联

#### 持久化

`acp_raw` **不写入 `events.jsonl`**（量级过大且为 live-only firehose）。因此：

- `get_logs` 不会回放 `acp_raw`——历史查询只能拿到投影事件
- 错过的 ACP 原始帧无法补取；客户端需在 run 开始前就订阅
- 这是有意取舍：保真透传服务于"实时旁观"，历史归档仍走轻量投影

---

## 4. 模块设计

### 文件结构

```
src/ws/
├── mod.rs          — AppState、axum 路由、serve() 公开函数
├── handler.rs      — WebSocket 连接处理（读写循环 + 订阅管理）
├── protocol.rs     — ClientMsg / ServerMsg 枚举（serde）
└── registry.rs     — RunRegistry：活跃 run 表（broadcast sender + cancel token）
```

`lib.rs` 新增 `pub mod ws;`（单行）。`cli::RunArgs` 增加 `pub events_tx: Option<EventSender>`（一个字段，向后兼容）。

**ACP 原始透传（§3.6）额外触及的现有文件**（仅在实现该可选扩展时）：

| 文件 | 改动 |
|------|------|
| `core/contract/event.rs` | `AgentEvent` 新增变体 `AcpRaw { run_id, agent_id, seq, update: serde_json::Value }` |
| `adapters/update_mapper.rs` | `handle_update` 在投影前，若 `emit_acp_raw` 开启则先发一条 `AcpRaw`（原样序列化 SessionUpdate + 递增 seq） |
| `adapters/acp_adapter.rs` | `AcpConfig` 增加 `emit_acp_raw: bool`，由 `serve --acp-raw` 注入；per-agent seq 计数器 |
| `cli.rs` | 事件→RunStore 转发任务跳过 `AcpRaw`（不持久化，见 §3.6）；无需改 `state.rs` |

> ⚠️ 这些改动使 footprint 超出"仅 events_tx 一个字段"的基础约束。若不需要全保真透传，跳过本表，基础协议不受影响。

---

### 4.1 `protocol.rs`

```rust
/// 客户端发来的消息
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Run         { id: String, payload: RunPayload },
    ConfirmRun  { id: String, payload: ConfirmRunPayload },
    Resume      { id: String, payload: IdPayload },
    Cancel      { id: String, payload: IdPayload },
    Subscribe   { id: String, payload: SubscribePayload },
    Unsubscribe { id: String, payload: IdPayload },
    GetStatus   { id: String, payload: IdPayload },
    ListRuns    { id: String, payload: ListRunsPayload },
    GetLogs     { id: String, payload: GetLogsPayload },
    GetFindings { id: String, payload: IdPayload },
    GetReport   { id: String, payload: IdPayload },
    Ping        { id: String },
}

#[derive(Debug, Deserialize)]
pub struct RunPayload {
    pub nl:       Option<String>,
    pub workflow: Option<PathBuf>,
    pub script:   Option<String>,
    #[serde(default)]
    pub args:     serde_json::Value,
    #[serde(default)]
    pub confirm:  bool,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmRunPayload {
    pub run_id:  RunId,
    pub approve: bool,
}

#[derive(Debug, Deserialize)]
pub struct SubscribePayload {
    pub run_id: RunId,
    /// None = 推送全部事件类型
    pub filter: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct ListRunsPayload {
    #[serde(default = "default_limit")]
    pub limit:  usize,
    #[serde(default)]
    pub offset: usize,
}

#[derive(Debug, Deserialize)]
pub struct GetLogsPayload {
    pub run_id: RunId,
    #[serde(default = "default_limit")]
    pub limit:  usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize { 20 }

/// 服务端发出的消息
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Hello         { version: &'static str, server: &'static str, capabilities: Vec<&'static str> },
    Accepted      { req_id: String, run_id: RunId },
    ScriptPreview { req_id: String, run_id: RunId, script: String },
    Event         { run_id: RunId, event: AgentEvent },
    Status        { req_id: String, run_id: RunId, data: StatusOutput },
    RunList       { req_id: String, total: usize, items: Vec<StatusOutput> },
    Logs          { req_id: String, run_id: RunId, total: usize, items: Vec<AgentEvent> },
    Findings      { req_id: String, run_id: RunId, items: Vec<Finding> },
    Report        { req_id: String, run_id: RunId, data: serde_json::Value },
    Ok            { req_id: String },
    Error         { req_id: String, code: ErrorCode, message: String },
    ServerClosing { reason: String },
    Pong          { req_id: String },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    NotFound,
    RunFinished,       // subscribe 时 run 已完成（区别于 not_found）
    AlreadyRunning,
    BackendError,
    Capacity,
    ConfirmTimeout,    // confirm_run 超时未到
    Internal,
}
```

---

### 4.2 `registry.rs`

```rust
/// 一个活跃 run 的控制句柄
pub struct RunHandle {
    /// 与 cli::run 内部相同的 broadcast sender，复用原始总线
    pub events: broadcast::Sender<AgentEvent>,
    /// CancellationToken，取消时调用 cancel.cancel()
    pub cancel: CancellationToken,
    /// 后台执行 task
    pub task:   JoinHandle<()>,
}

/// 活跃 run 注册表（DashMap，支持并发读写）
#[derive(Clone, Default)]
pub struct RunRegistry(Arc<DashMap<RunId, RunHandle>>);

impl RunRegistry {
    pub fn insert(&self, id: RunId, handle: RunHandle);
    /// 订阅事件流；run 不存在返回 None
    pub fn subscribe(&self, id: &RunId) -> Option<broadcast::Receiver<AgentEvent>>;
    /// 发出取消信号；run 不存在返回 false
    pub fn cancel(&self, id: &RunId) -> bool;
    /// run 结束后清理（由后台 task 完成回调调用）
    pub fn remove(&self, id: &RunId);
    /// 关闭所有活跃 run（服务端关闭时调用）
    pub fn cancel_all(&self);
}
```

`DashMap` 已在项目中使用（`dashmap = "6"`），无需新增依赖。

---

### 4.3 `mod.rs`

```rust
/// 服务端全局状态（所有连接共享，Clone 廉价）
#[derive(Clone)]
pub struct AppState {
    pub backend:          Arc<dyn AgentBackend>,
    pub registry:         RunRegistry,
    pub base_dir:         PathBuf,          // 默认 ./.maestro/runs
    pub run_permits:      Arc<Semaphore>,   // 限制并发 run 数，默认 4
    pub confirm_timeout:  Duration,         // script_preview 等待确认超时，默认 30s
    pub acp_raw:          bool,             // 是否启用 ACP 原始透传（§3.6），默认 false
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/ws",     get(ws_handler))
        .route("/health", get(health_handler))
        .layer(CorsLayer::new()
            .allow_origin(["http://localhost", "http://127.0.0.1"])
            .allow_methods([Method::GET]))
        .with_state(state)
}

/// 启动服务，返回实际绑定地址（供测试随机端口用）
pub async fn serve(state: AppState, addr: SocketAddr) -> Result<SocketAddr>;
```

`/health` 返回 `{ "ok": true, "version": "0.1.0" }`，供容器探活。

---

### 4.4 `handler.rs`

连接建立后采用**双任务模型**：

```
handle_ws()
│
├── 立即发送 ServerMsg::Hello
│
├── out_tx / out_rx  (mpsc channel，容量 64)
│
├── 写任务 (tokio::spawn)
│     out_rx → 序列化 → WebSocket sink
│
└── 读/事件循环（主循环）
      conn_state: ConnState {
          subscriptions: HashMap<RunId, SubscribePayload>,
          events_merged: SelectAll<FilteredStream>,  // 含 filter 逻辑
          pending_confirms: HashMap<RunId, (String/*script*/, Instant)>,
      }
      │
      tokio::select! {
          // 来自客户端的控制消息
          Some(Ok(msg)) = stream.next()  → dispatch_client_msg()
          // 来自所有订阅 run 的实时事件
          Some(evt) = events_merged.next() → out_tx.send(ServerMsg::Event)
          // confirm 超时检查（每 5s tick 一次）
          _ = timeout_ticker.tick()      → check_confirm_timeouts()
      }
```

`FilteredStream` 是对 `BroadcastStream<AgentEvent>` 的轻量包装，在推送前检查事件类型是否在 `filter` 列表中，过滤在服务端完成，避免把高频 `agent_progress` 事件序列化后再丢弃。

`dispatch_client_msg` 分派表：

| ClientMsg | 动作 |
|-----------|------|
| `Run` | 验证 payload → 申请 permit → 注册 RunHandle → 回复 Accepted → 后台 task：NL 分支先 `plan_workflow` 规划，再以 `script` 构建 RunArgs（含 events_tx）调 cli::run；`confirm:true` 则改为回复 ScriptPreview 并记录 pending_confirm |
| `ConfirmRun` | 检查 pending_confirm → approve: true 则触发执行，false 则清理 → 回复 Ok/Error |
| `Resume` | 验证 run 存在且可恢复 → 申请 permit → spawn cli::run(resume) → 注册 → 回复 Accepted |
| `Cancel` | `registry.cancel(run_id)`：命中→Ok；miss→回退查磁盘区分 Error(RunFinished)/Error(NotFound) |
| `Subscribe` | `registry.subscribe`：命中→加入 events_merged→Ok；miss→回退查磁盘 Error(RunFinished)/Error(NotFound) |
| `Unsubscribe` | 从 events_merged 移除 → 回复 Ok |
| `GetStatus` | 读 RunStore checkpoint → 投影 StatusOutput → 回复 Status |
| `ListRuns` | `cli::list_runs_cmd` → 分页 → 回复 RunList |
| `GetLogs` | `cli::logs_cmd` → 反序列化为 AgentEvent → 分页 → 回复 Logs |
| `GetFindings` | `cli::findings_cmd` → 回复 Findings |
| `GetReport` | 扫描 events.jsonl 最后一条 run_done → 取 report → 回复 Report/Error(NotFound) |
| `Ping` | 回复 Pong |

---

## 5. 核心流程

### 5.1 提交并订阅 run（标准流程）

> **关键前提**：`cli::run` **不做 NL→脚本规划**——它只接受 `script` / `workflow` / `--resume`，传入纯 `nl` 会直接 bail（`cli.rs`）。因此 **NL 规划必须在 WS 层完成**：先调 `planner::plan_workflow(nl, backend)` 生成 Lua 脚本，再以 `RunArgs.script` 传给 `cli::run`。`confirm:false` 与 `confirm:true` 的唯一区别是是否插入 `script_preview` 往返，规划步骤两者都不能省。

```
客户端: run { nl: "...", confirm: false }
         │
         ▼
① 验证：nl/workflow/script 恰好一个非空；否则 Error(BadRequest)
② try_acquire run_permits；满载时 Error(Capacity)
③ run_id = Uuid::now_v7()
④ broadcast::channel::<AgentEvent>(256) → 保留 events_tx，丢弃首个 receiver
⑤ cancel = CancellationToken::new()
⑥ task = tokio::spawn(async move {
       // 闭包提前捕获 run_id / registry / events_tx.clone() / cancel.clone() / permit
       // NL 分支：先在 WS 层规划（plan_workflow 是 async，无需 spawn_blocking）
       let script = match run_payload {
           nl       => planner::plan_workflow(nl, backend, &cfg).await?.script,
           script   => script,            // 内嵌脚本直接用
           workflow => fs::read(workflow)?,// 文件路径读盘
       };
       // 规划失败 → 通过 events_tx 发 run_done{failed} 通知订阅者
       let run_args = RunArgs { script: Some(script),
                                events_tx: Some(events_tx), ... };
       cli::run(backend, run_args).await.ok();
       registry.remove(&run_id);          // 见下方「条目生命周期」
       drop(permit);
   })
⑦ registry.insert(run_id, RunHandle { events: events_tx.clone(), cancel, task })
   （insert 在 spawn 之后——RunHandle 需要 ⑥ 返回的 JoinHandle）
⑧ → 立即回复 Accepted { run_id }
     （规划 + 执行在后台异步进行，run_id 此刻即可用于 subscribe）

客户端: subscribe { run_id, filter: ["agent_done", "run_done"] }
         │
         ▼
① registry.subscribe(run_id) → Receiver<AgentEvent>
② FilteredStream::new(rx, filter) 加入 events_merged
③ → 回复 Ok

后续推送: event { run_id, event: { type: "agent_done", ... } }
          event { run_id, event: { type: "run_done",   ... } }
```

**`accepted` 时序**：`run_id` 在规划开始前就生成并立即回复，客户端可在规划期间就 `subscribe`。NL 规划耗时数秒，期间不阻塞 `accepted`；规划本身（`plan_workflow`）若失败，后台 task 通过 `events_tx` 发出 `run_done { status: "failed" }` + `log` 事件告知订阅者。

**`events_tx` 注入点**：`cli::run` 检查 `args.events_tx`，若 `Some` 则直接用该 sender 构建 `RunContext`，不新建 broadcast channel。这是对 `cli.rs` 的唯一修改，`None` 时行为与现在完全相同。

> ⚠️ **规划与执行共用一个 broadcast sender**：步骤④的 `events_tx` 在回复 Accepted 前就存入 RunRegistry（步骤⑦），使客户端能在规划阶段订阅。但注意 `plan_workflow` 自身不发 `AgentEvent`（它通过独立的 `PlanningState` 回调上报进度），所以规划阶段订阅者只会在脚本开始执行后才收到 `run_started` 等事件。

**条目生命周期与 `run_finished` 检测**：RunHandle 在 run 结束时即从 RunRegistry 移除（步骤⑥的 `registry.remove`），registry 只保存**活跃** run。因此 `subscribe` / `cancel` 在 registry miss 时需**回退查磁盘**来区分两种 NotFound 语义：

```
registry.get(run_id)
├── Some(handle)              → 活跃，正常 subscribe / cancel
└── None
    ├── .maestro/runs/<id>/ 存在  → run 已结束 → Error(RunFinished)
    └── 目录不存在               → run 从未存在 → Error(NotFound)
```

这是 `run_finished` 与 `not_found` 两个错误码的唯一判别依据——registry 不保留已结束条目，磁盘 run 目录是已结束 run 的权威记录。

---

### 5.2 NL run 脚本预览与确认

```
客户端: run { nl: "...", confirm: true }
         │
         ▼
① 申请 permit，创建 run_id（同上）
② planner::plan_workflow(nl, backend, &cfg).await 生成 Lua 脚本
   （plan_workflow 是 async，直接 await 即可——无需 spawn_blocking；
    只有后续的 mlua rt.execute 才需要阻塞线程，那发生在 cli::run 内部）
③ → 回复 ScriptPreview { run_id, script: planned.script }
④ pending_confirms.insert(run_id, (script, Instant::now()))
   （等待 confirm_timeout，默认 30s）

客户端（审查脚本后）: confirm_run { run_id, approve: true }
         │
         ▼
⑤ pending_confirms.remove(run_id) → 取得 script
⑥ RunArgs { script: Some(script), events_tx: Some(events_tx), ... }
⑦ spawn cli::run → RunRegistry → 回复 Ok

超时（30s 内未收到 confirm_run）:
⑧ pending_confirms 清理，permit 归还
⑨ 若该连接仍活跃，回复 Error(ConfirmTimeout)
```

---

### 5.3 取消 run

```
客户端: cancel { run_id }
         │
         ▼
① registry.cancel(run_id)
   → RunHandle.cancel.cancel()
   → Scheduler 的 CancellationToken 触发，中止所有 agent
② → 回复 Ok（仅表示信号已发，run 尚未终止）

异步：cli::run 收到取消信号
   → RunDone { status: Cancelled }
   → 所有订阅者收到 event { run_done { status: "cancelled" } }
   → RunRegistry.remove 清理
```

> **异步性说明**：`ok` 只意味着"取消信号已发出"，不意味着 run 已停止。客户端应等待 `run_done` 事件确认 run 真正结束。

---

### 5.4 服务端关闭

```
Ctrl-C 信号
    │
    ▼
① registry.cancel_all() — 向所有活跃 run 发取消信号
② 向所有 WebSocket 连接广播 server_closing { reason: "shutdown" }
③ 等待活跃 run 完成（最多 grace_period，默认 5s）
④ 关闭所有 WebSocket 连接
⑤ HTTP server shutdown
```

---

### 5.5 断线重连

服务端不维护连接状态。客户端重连后：

- **run 仍在运行**：重新发 `subscribe { run_id }`，即可恢复实时事件流（从重连时刻起）
- **run 已完成**：`subscribe` 返回 `Error(RunFinished)`，改用 `get_logs` + `get_report` + `get_findings` 补取完整结果
- **重连前错过的事件**：从 `get_logs { offset: 已收到条数 }` 分页补取

---

## 6. CLI 集成

### `RunArgs` 修改（`cli.rs`）

```rust
pub struct RunArgs {
    // ... 现有字段不变 ...

    /// WebSocket 服务端注入的事件总线；None 时 cli::run 内部自建（向后兼容）
    pub events_tx: Option<crate::core::contract::event::EventSender>,
}
```

`cli::run` 中原本的：

```rust
let (events_tx, _events_rx) = tokio::sync::broadcast::channel(256);
```

改为：

```rust
let events_tx = args.events_tx.unwrap_or_else(|| {
    tokio::sync::broadcast::channel(256).0
});
```

### `main.rs` 新增 `Serve` 子命令

```rust
/// 启动 WebSocket 服务器
Serve {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(short, long, default_value_t = 7474)]
    port: u16,

    #[arg(short, long)]
    backend: Option<String>,

    #[arg(long, default_value_t = 4)]
    max_runs: usize,

    /// 启用 ACP 原始事件透传（acp_raw 事件，见 §3.6）；默认关闭
    #[arg(long, default_value_t = false)]
    acp_raw: bool,
}
```

分派：

```rust
Commands::Serve { host, port, backend, max_runs, acp_raw } => {
    // acp_raw 注入 backend 工厂：开启时 AcpConfig.emit_acp_raw = true
    let backend = backend::create_backend_with_raw(
        backend.as_deref().unwrap_or_else(|| backend::detect_backend()),
        acp_raw,
    )?;
    let state = maestro::ws::AppState {
        backend,
        registry:        Default::default(),
        base_dir:        PathBuf::from(".maestro/runs"),
        run_permits:     Arc::new(Semaphore::new(max_runs)),
        confirm_timeout: Duration::from_secs(30),
        acp_raw,                                  // 控制 hello.capabilities 与默认 filter
    };
    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    let bound = maestro::ws::serve(state, addr).await?;
    println!("maestro ws server  ws://{}/ws", bound);
    println!("health check       http://{}/health", bound);
    tokio::signal::ctrl_c().await?;
    state.registry.cancel_all();
    Ok(())
}
```

---

## 7. 错误处理

### 错误码

| `code` | HTTP 类比 | 触发场景 |
|--------|-----------|---------|
| `bad_request` | 400 | JSON 解析失败；`nl`/`workflow`/`script` 均为空或同时多个非空 |
| `not_found` | 404 | `run_id` 既不在 RunRegistry，磁盘 `.maestro/runs/<id>/` 也不存在（从未创建） |
| `run_finished` | 410 | `subscribe`/`cancel` 时 registry miss 但磁盘 run 目录存在（run 已结束）；改用 `get_logs`/`get_report` |
| `already_running` | 409 | resume 目标 run 当前已在运行 |
| `backend_error` | 502 | backend 无法启动（如 opencode 未安装） |
| `capacity` | 503 | 已达 `max_runs` 上限 |
| `confirm_timeout` | 408 | `script_preview` 发出后 30s 内未收到 `confirm_run` |
| `internal` | 500 | 其他未预期错误 |

### 消息解析失败

帧无法解析为 `ClientMsg` 时，发 `error(bad_request)` 并**保持连接**，允许客户端修正后重试。

### 广播滞后（Lagged）

broadcast channel 容量 256。客户端消费过慢导致 `RecvError::Lagged(n)` 时，注入一条 Log 事件通知客户端事件丢失，不断开连接：

```json
{
  "type":   "event",
  "run_id": "...",
  "event": {
    "type":  "log",
    "level": "warn",
    "msg":   "[ws] broadcast lagged: 12 events dropped for this subscriber"
  }
}
```

客户端可用 `get_logs` 补查丢失的事件。

---

## 8. 安全设计

### 绑定地址

默认 `127.0.0.1`，仅接受本机连接。如需对外暴露（如 Docker），显式传 `--host 0.0.0.0` + 在上层做网络隔离。

### CORS

```rust
CorsLayer::new()
    .allow_origin([
        "http://localhost".parse::<HeaderValue>().unwrap(),
        "http://127.0.0.1".parse::<HeaderValue>().unwrap(),
    ])
    .allow_methods([Method::GET])
```

### 消息大小限制

拒绝超过 **64 KB** 的单帧（axum ws `max_frame_size`），防止超大 `script` 内嵌绕过文件路径验证。内嵌脚本支持仅用于调试场景，生产使用应优先 `workflow` 文件路径。

### 并发限制

```rust
let Ok(permit) = state.run_permits.try_acquire_owned() else {
    return out_tx.send(ServerMsg::Error {
        req_id, code: ErrorCode::Capacity,
        message: "max concurrent runs reached".into(),
    }).await;
};
```

permit 在 run 结束后由后台 task 的 `drop(permit)` 自动归还。

### 认证（v0.1 不实现）

本地工具场景依赖绑定 localhost。如未来需要：在 axum 层加 `tower_http::validate_request` 中间件比对 `Authorization: Bearer <token>`，token 在 `AppState` 中初始化时生成并打印到 stdout。

---

## 9. 依赖增量

仅增加两个 crate：

```toml
axum       = { version = "0.7", features = ["ws"] }
tower-http = { version = "0.5", features = ["cors"] }
```

已有依赖复用：

| 已有 crate | 用途 |
|-----------|------|
| `tokio`（full） | spawn、Semaphore、broadcast、signal、time |
| `tokio-stream 0.1` | `BroadcastStream` wrapper |
| `futures 0.3` | `SelectAll` 合并多路事件流 |
| `dashmap 6` | `RunRegistry` 内部并发 Map |
| `tokio-util 0.7` | `CancellationToken` |
| `serde` / `serde_json` | 消息序列化 |
| `uuid`(v7) | `run_id` 生成 |

---

## 10. 实现计划

| 阶段 | 内容 | 文件 | 估时 |
|------|------|------|------|
| **P1** | 协议与数据结构（纯 serde，可单元测试） | `protocol.rs`、`registry.rs` | 1.5 h |
| **P2** | axum 路由 + serve + `/health` | `mod.rs` | 1 h |
| **P3** | 连接处理：hello + ping/pong + get_status/list_runs/get_logs/get_findings/get_report | `handler.rs` | 2 h |
| **P4** | run / cancel / subscribe / unsubscribe + FilteredStream | `handler.rs` | 2 h |
| **P5** | script_preview + confirm_run + resume + server_closing | `handler.rs` | 1.5 h |
| **P6** | `RunArgs.events_tx` 注入 + `main.rs` Serve 子命令 | `cli.rs`、`main.rs`、`lib.rs` | 1 h |
| **P7** | 集成测试 | `tests/ws_integration.rs` | 2 h |
| **P8**（可选） | ACP 原始透传（§3.6）：`AcpRaw` 变体 + 适配层 emit + seq + serve `--acp-raw` + filter | `event.rs`、`update_mapper.rs`、`acp_adapter.rs`、`cli.rs` | 2.5 h |

基础协议 P1–P7 约 **11 h**；含可选 P8 约 **13.5 h**。P1–P3 可独立完成并验证（P2 用 curl 探活，P3 用 `wscat` 手测只读路径）。P8 独立于基础协议，可后置。

### P7 集成测试策略

```rust
// tests/ws_integration.rs（以 MockBackend 启动，绑定随机端口）
#[tokio::test]
async fn test_hello_on_connect() { /* 验证连接后立即收到 hello */ }

#[tokio::test]
async fn test_run_subscribe_and_events() { /* run → accepted → subscribe → events → run_done */ }

#[tokio::test]
async fn test_script_preview_approve() { /* run(confirm:true) → script_preview → confirm → run_done */ }

#[tokio::test]
async fn test_script_preview_reject() { /* confirm(approve:false) → ok，run 被清理 */ }

#[tokio::test]
async fn test_cancel_run() { /* cancel → ok → run_done(cancelled) */ }

#[tokio::test]
async fn test_subscribe_filter() { /* filter 只含 run_done，验证不收到 agent_started */ }

#[tokio::test]
async fn test_subscribe_finished_run() { /* run 完成后 subscribe → error(run_finished) */ }

#[tokio::test]
async fn test_list_runs_and_get_logs() { /* list_runs → run_list；get_logs → logs */ }

#[tokio::test]
async fn test_capacity_limit() { /* 超过 max_runs → error(capacity) */ }

#[tokio::test]
async fn test_bad_request() { /* 发送无效 JSON → error(bad_request)，连接保持 */ }
```

测试额外依赖：`tokio-tungstenite`（仅 dev-dependencies）。

---

## 11. 已知局限与后续工作

- **实时事件回放**：`subscribe` 成功时只能收到之后的新事件，之前已广播的事件已丢失。客户端可先 `get_logs { offset: 0 }` 拉取已落盘事件，再 `subscribe` 接续实时流（存在短暂重叠，客户端需去重）。
- **`get_logs` 分页的 offset 语义**：当前按条数分页，在 run 进行中时 `total` 可能随时变化，客户端应以 offset + items 长度推进，而非依赖 total。
- **认证与 TLS**：v0.1 依赖绑定 localhost。生产部署需补充 token 验证和 `rustls`。
- **workflow 文件路径安全**：`workflow` 字段接受任意绝对路径，服务端应校验路径在允许的根目录内（path traversal 防御），v0.1 先记录为 TODO。
- **confirm_timeout 时钟**：当前用 ticker 轮询检查，精度 5s。如需更精确，改为每个 pending_confirm 独立 `tokio::time::sleep`。
- **`resume` 的 run_id 发现**：`resume` 目前要求客户端自行知道 `run_id`（先 `list_runs` 获取），无"恢复最近一次"的快捷路径。
- **多服务端实例**：RunRegistry 是进程内 DashMap，不支持多进程共享。分布式场景需外部状态存储（超出 v0.1 范围）。
- **ACP 保真度**：默认投影事件（`agent_progress`）有损——reasoning 并入 message、工具 input/结果/Plan 被丢弃。[§3.6 ACP 原始透传](#36-acp-原始透传可选-opt-in)以 opt-in 方式提供全保真流，但 `acp_raw` 仅 live、不持久化、不可历史补取，且只有 ACP 后端（opencode）会产生——mock 后端无原始流。非 ACP 后端开启 `--acp-raw` 时 `acp_raw` 始终为空。

---

## 相关文档

- 架构总览：[../architecture.md](../architecture.md)
- CLI 模块：[../architecture/cli.md](../architecture/cli.md)（run 生命周期、RunArgs、RunMode）
- 事件合约：[`src/core/contract/event.rs`](../../src/core/contract/event.rs)（AgentEvent 完整定义）
- Finding 合约：[`src/core/contract/finding.rs`](../../src/core/contract/finding.rs)（Finding 结构）
- MCP 数据面：[../architecture/mcp.md](../architecture/mcp.md)（另一种程序化接口，stdio JSON-RPC）
- WS 协议测试：[ws-test.md](ws-test.md)（`maestro test-ws` 命令，按场景验证协议）
