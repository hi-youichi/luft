# Agent 交互结构化持久化方案

> **状态**: 方案设计
> **目标**: 将 agent 的工具调用、reasoning、助手消息等完整交互数据结构化保存，使 UI 能够高保真地回放和浏览
> **交叉参考**: [事件日志](./event-logging.md)、[程序日志](./program-logging.md)、[SDK 事件](./sdk-events.md)、[ACP 原始事件](./acp-raw-events.md)
> **相关代码**: [`src/core/contract/event.rs`](../../src/core/contract/event.rs)、[`src/adapters/update_mapper.rs`](../../src/adapters/update_mapper.rs)、[`src/core/journal.rs`](../../src/core/journal.rs)、[`src/service/query.rs`](../../src/service/query.rs)

---

## 0. 问题：当前持久化丢掉了什么

Luft 已有一套事件总线 + JSONL 持久化体系，但为 UI 场景做结构化回放时，存在三层信息损耗：

### 0.1 ProgressDelta 信息太薄

当前 `ProgressDelta`（[`event.rs:166-173`](../../src/core/contract/event.rs#L166-L173)）是 agent 交互的核心载体，但字段严重不足：

| Delta 变体 | 当前字段 | 丢失的信息 |
|---|---|---|
| `Message` | `text: String` | **无 role 区分**：assistant / reasoning(thinking) 全靠 `[reasoning]` 前缀 hack（[`update_mapper.rs:67-68`](../../src/adapters/update_mapper.rs#L67-L68)）；无消息 ID，无法关联 chunk |
| `ToolCall` | `name: String, summary: String` | **无 tool_call_id**：无法关联 `ToolCall` 与后续的 `ToolCallUpdate`；`summary` 实际存的是 `kind`（工具类别），不是摘要；**无 input/output** |
| `FileEdit` | `path: PathBuf` | **无操作类型**（create/edit/delete）；**无 diff 内容** |
| `Tokens` | `usage: TokenUsage` | OK，信息完整 |

### 0.2 AcpRaw 被丢弃

`AcpRaw` 事件包含完整的 ACP `session/update` 原始 payload（含 `rawInput`、`ToolCallUpdateFields` 等），但在 forwarder 中被**显式过滤掉不落盘**（[`service/run.rs:259`](../../src/service/run.rs#L259)）。这意味着最丰富的交互细节（工具入参、输出、plan）在持久化层完全消失。

### 0.3 缺少时间戳与序号

- `AgentProgress` 事件**没有 `ts` 字段**——UI 无法构建时间线
- 所有事件**没有全局递增序号**——并发写入时文件位置不保证因果序
- 消息 chunk 之间**无关联 ID**——无法将流式 chunk 组装回完整消息

---

## 1. 设计理念

- **结构化优先**：所有交互数据以强类型字段存储，不依赖字符串前缀或 JSON 挖掘
- **向后兼容**：扩展现有 `ProgressDelta`，不引入新的持久化格式；旧 JSONL 可正常反序列化
- **分层可选**：基础层（轻量，默认开启）捕获足够的 UI 元数据；增强层（可选）保留完整 payload
- **UI-ready**：数据模型直接映射为 UI 的 conversation/thread 视图，查询层提供聚合 API

---

## 2. 数据模型扩展

### 2.1 MessageRole 枚举（新增）

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    Assistant,
    Reasoning,
    User,
}
```

### 2.2 ProgressDelta 扩展

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProgressDelta {
    // ── 扩展 ──
    Message {
        role: MessageRole,          // 新增：区分 assistant / reasoning
        text: String,
        message_id: Option<String>, // 新增：ACP content chunk group ID，用于组装流式 chunk
    },
    ToolCall {
        tool_call_id: String,       // 新增：ACP tool call ID，关联 call → update → result
        name: String,               // 重命名：title → name（工具名称）
        input: Option<serde_json::Value>,  // 新增：工具入参（来自 ACP rawInput）
    },
    ToolResult {                     // 新增变体：工具执行结果
        tool_call_id: String,
        status: ToolStatus,         // Completed / Failed
        output: Option<serde_json::Value>,
        elapsed_ms: u64,
    },
    FileEdit {
        path: PathBuf,
        op: FileOp,                 // 新增：Create / Edit / Delete
        diff: Option<String>,       // 新增：unified diff（来自 ToolCallUpdate）
    },
    Tokens {
        usage: TokenUsage,
    },
}
```

### 2.3 辅助枚举

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileOp {
    Create,
    Edit,
    Delete,
}
```

### 2.4 AgentProgress 补时间戳

```rust
AgentProgress {
    run_id: RunId,
    agent_id: AgentId,
    delta: ProgressDelta,
    ts: DateTime<Utc>,      // 新增
    seq: u64,               // 新增：全局递增序号
},
```

> **向后兼容策略**：`ts` 和 `seq` 使用 `#[serde(default)]`，旧 JSONL 反序列化时 `ts = None`（改为 `Option<DateTime<Utc>>`）或使用 epoch 默认值。`seq` 默认 0。

---

## 3. update_mapper 改造

### 3.1 AgentMessageChunk

```rust
// 当前（update_mapper.rs:53-58）
SessionUpdate::AgentMessageChunk(chunk) => {
    if let Some(text) = json_find_text(chunk) {
        acc.message.lock().unwrap().push_str(&text);
        emit(events, run_id, agent_id, ProgressDelta::Message { text });
    }
}

// 改造后
SessionUpdate::AgentMessageChunk(chunk) => {
    if let Some(text) = json_find_text(chunk) {
        acc.message.lock().unwrap().push_str(&text);
        let message_id = extract_message_id(chunk);  // 从 ACP chunk 提取 group ID
        emit(events, run_id, agent_id, ProgressDelta::Message {
            role: MessageRole::Assistant,
            text,
            message_id,
        });
    }
}
```

### 3.2 AgentThoughtChunk

```rust
// 当前（update_mapper.rs:60-71）—— 用 [reasoning] 前缀 hack
SessionUpdate::AgentThoughtChunk(chunk) => {
    if let Some(text) = json_find_text(chunk) {
        emit(events, run_id, agent_id, ProgressDelta::Message {
            text: format!("[reasoning] {text}"),  // ← hack
        });
    }
}

// 改造后 —— 结构化 role
SessionUpdate::AgentThoughtChunk(chunk) => {
    if let Some(text) = json_find_text(chunk) {
        let message_id = extract_message_id(chunk);
        emit(events, run_id, agent_id, ProgressDelta::Message {
            role: MessageRole::Reasoning,
            text,
            message_id,
        });
    }
}
```

### 3.3 ToolCall

```rust
// 当前（update_mapper.rs:73-89）—— 只存 title + kind
SessionUpdate::ToolCall(tc) => {
    let v = to_json(tc);
    let title = find_str(&v, "title").filter(|s| !s.is_empty()).unwrap_or("tool");
    let kind = find_str(&v, "kind").unwrap_or_default();
    // ... structured_output 提取 ...
    emit(events, run_id, agent_id, ProgressDelta::ToolCall { name: title, summary: kind });
}

// 改造后 —— 存 tool_call_id + name + input
SessionUpdate::ToolCall(tc) => {
    let v = to_json(tc);
    let tool_call_id = find_str(&v, "id").unwrap_or_else(|| format!("tc-{}", next_id()));
    let name = find_str(&v, "title").filter(|s| !s.is_empty()).unwrap_or_else(|| "tool".to_string());
    let input = v.get("rawInput").cloned()
        .or_else(|| v.get("raw_input").cloned())
        .filter(|v| !v.is_null());

    // ... structured_output 提取保持不变 ...

    emit(events, run_id, agent_id, ProgressDelta::ToolCall {
        tool_call_id,
        name,
        input,
    });
}
```

### 3.4 ToolCallUpdate

```rust
// 当前（update_mapper.rs:91-113）—— 只存 path
SessionUpdate::ToolCallUpdate(u) => {
    let v = to_json(u);
    // ... structured_output 提取 ...
    if let Some(path) = find_str(&v, "path") {
        emit(events, run_id, agent_id, ProgressDelta::FileEdit { path: path.into() });
    }
}

// 改造后 —— 存 tool_call_id + op + diff，并可发 ToolResult
SessionUpdate::ToolCallUpdate(u) => {
    let v = to_json(u);
    let tool_call_id = find_str(&v, "id").unwrap_or_default();
    let status = find_str(&v, "status");  // ACP 的 ToolCallStatus: completed/failed

    // 文件编辑事件
    if let Some(path) = find_str(&v, "path") {
        let op = infer_file_op(&v);  // 从 ACP fields 推断 create/edit/delete
        let diff = find_str(&v, "diff").or_else(|| find_str(&v, "content"));
        emit(events, run_id, agent_id, ProgressDelta::FileEdit {
            path: path.into(),
            op,
            diff,
        });
    }

    // 工具完成事件
    if let Some(ref st) = status {
        let tool_status = match st.as_str() {
            "failed" => ToolStatus::Failed,
            _ => ToolStatus::Completed,
        };
        let output = v.get("output").cloned().filter(|v| !v.is_null());
        emit(events, run_id, agent_id, ProgressDelta::ToolResult {
            tool_call_id,
            status: tool_status,
            output,
            elapsed_ms: 0,  // ACP update 不携带耗时；可从 Started→Done 间隔推导
        });
    }
}
```

---

## 4. 持久化层改动

### 4.1 事件序列号

在 `EventSender` 的上游（`service/run.rs` 的 forwarder）或 `JournalStore` 层注入全局递增 `seq`：

```rust
// journal.rs — append_event 时自动分配 seq
pub fn append_event(&self, mut event: AgentEvent) -> Result<()> {
    let seq = self.seq_counter.fetch_add(1, Ordering::Relaxed);
    if let AgentEvent::AgentProgress { ref mut seq: s, .. } = event {
        *s = seq;
    }
    // ... 写入 events.jsonl ...
}
```

### 4.2 AcpRaw 选择性落盘

当前 `AcpRaw` 被 forwarder 全量跳过（[`service/run.rs:259`](../../src/service/run.rs#L259)）。方案：

- **不改变默认行为**（保持跳过，避免膨胀）
- 新增配置 `persist_acp_raw: bool`，开启后 `AcpRaw` 也进 `events.jsonl`
- 或者：放弃 `AcpRaw` 落盘，改为在 `update_mapper` 中把关键信息提取进结构化的 `ProgressDelta` 字段（本方案推荐路线）

### 4.3 存储格式不变

仍然是 `events.jsonl`（每行一个 JSON），只是每行的 `AgentProgress` 事件 payload 更丰富。示例：

```jsonl
{"type":"agent_started","run_id":"...","phase_id":"...","agent_id":"...","prompt_preview":"...","model":"claude-sonnet-4"}
{"type":"agent_progress","run_id":"...","agent_id":"...","ts":"2025-08-19T10:00:01Z","seq":3,"delta":{"kind":"message","role":"reasoning","text":"Let me analyze the code...","message_id":null}}
{"type":"agent_progress","run_id":"...","agent_id":"...","ts":"2025-08-19T10:00:02Z","seq":4,"delta":{"kind":"tool_call","tool_call_id":"tc-1","name":"ReadFile","input":{"path":"src/main.rs"}}}
{"type":"agent_progress","run_id":"...","agent_id":"...","ts":"2025-08-19T10:00:03Z","seq":5,"delta":{"kind":"tool_result","tool_call_id":"tc-1","status":"completed","output":{"content":"..."},"elapsed_ms":120}}
{"type":"agent_progress","run_id":"...","agent_id":"...","ts":"2025-08-19T10:00:04Z","seq":6,"delta":{"kind":"file_edit","path":"src/main.rs","op":"edit","diff":"@@ -1,3 +1,4 @@"}}
{"type":"agent_progress","run_id":"...","agent_id":"...","ts":"2025-08-19T10:00:05Z","seq":7,"delta":{"kind":"message","role":"assistant","text":"I've updated the file.","message_id":null}}
{"type":"agent_done","run_id":"...","agent_id":"...","status":"ok","tokens":{"input":5000,"output":800,"cache_read":0,"cache_write":0},"elapsed_ms":5000}
```

---

## 5. 查询层（UI-Ready API）

在 `src/service/query.rs` 新增聚合查询函数，将扁平事件流重组为 UI 友好的视图模型。

### 5.1 Conversation 视图

将一个 agent 的所有 `AgentProgress` 事件按 `seq` 排序，重组为有序的 conversation turn 列表：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub seq: u64,
    pub ts: Option<DateTime<Utc>>,
    pub agent_id: AgentId,
    pub turn_type: TurnType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnType {
    Message { role: MessageRole, text: String },
    ToolCall { tool_call_id: String, name: String, input: Option<serde_json::Value> },
    ToolResult { tool_call_id: String, status: ToolStatus, output: Option<serde_json::Value> },
    FileEdit { path: PathBuf, op: FileOp, diff: Option<String> },
}

pub fn get_agent_conversation(run_id: &RunId, agent_id: &AgentId) -> Result<Vec<ConversationTurn>>;
```

### 5.2 Run 概览视图

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOverview {
    pub run_id: RunId,
    pub task: String,
    pub status: RunStatus,
    pub started_ts: Option<DateTime<Utc>>,
    pub elapsed_ms: u64,
    pub total_tokens: TokenUsage,
    pub phases: Vec<PhaseOverview>,
    pub agents: Vec<AgentOverview>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOverview {
    pub agent_id: AgentId,
    pub phase_id: PhaseId,
    pub model: Option<String>,
    pub status: AgentStatus,
    pub tokens: TokenUsage,
    pub elapsed_ms: u64,
    pub message_count: usize,
    pub tool_call_count: usize,
    pub file_edit_count: usize,
}

pub fn get_run_overview(run_id: &RunId) -> Result<RunOverview>;
```

### 5.3 Run 树视图

重建编排层级（phase → parallel/converge/workflow → agent），用于 UI 的树状导航：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunTree {
    pub run_id: RunId,
    pub task: String,
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TreeNode {
    Phase { phase_id: PhaseId, label: String, children: Vec<TreeNode> },
    Agent { agent_id: AgentId, model: Option<String>, status: AgentStatus },
    Parallel { span_id: u64, children: Vec<TreeNode> },
    Converge { span_id: u64, rounds: u32, children: Vec<TreeNode> },
    Workflow { span_id: u64, path: String, children: Vec<TreeNode> },
}

pub fn get_run_tree(run_id: &RunId) -> Result<RunTree>;
```

---

## 6. 改动文件清单

| 文件 | 改动 | 兼容性 |
|---|---|---|
| [`event.rs`](../../src/core/contract/event.rs) | 扩展 `ProgressDelta`（加字段 + 新变体 `ToolResult`）；`AgentProgress` 加 `ts` + `seq`；新增 `MessageRole`/`ToolStatus`/`FileOp` | `#[serde(default)]` 保证旧 JSONL 可读 |
| [`update_mapper.rs`](../../src/adapters/update_mapper.rs) | 改造 5 个 `SessionUpdate` 分支的映射逻辑 | 行为变更（更好的数据），无 API break |
| [`journal.rs`](../../src/core/journal.rs) | `append_event` 注入 `seq`；`seq_counter: AtomicU64` | 自动处理，调用方无感 |
| [`service/run.rs`](../../src/service/run.rs) | forwarder 可选注入 `ts`（如果不在 journal 层做） | — |
| [`query.rs`](../../src/service/query.rs) | 新增 `get_agent_conversation`/`get_run_overview`/`get_run_tree` | 纯新增函数 |
| [`protocol.rs`](../../src/adapters/protocol.rs) | `event_type_name`/`event_run_id` 匹配新 `ToolResult` 变体 | 编译器强制 |

**无需改动**：`event_log.rs`（EventLogger 天然序列化新字段）、`subscription.rs`（`passes_filter` 已放行 `AgentProgress`）

---

## 7. 向后兼容策略

| 场景 | 策略 |
|---|---|
| 旧 JSONL 反序列化 | `ProgressDelta::Message` 缺 `role` → `#[serde(default)]` = `Assistant`；缺 `message_id` → `None` |
| 旧 JSONL 反序列化 | `ProgressDelta::ToolCall` 缺 `tool_call_id` → `#[serde(default)]` = `""`；缺 `input` → `None` |
| 旧 JSONL 反序列化 | `AgentProgress` 缺 `ts`/`seq` → `#[serde(default)]` |
| `summary` 字段移除 | 旧格式有 `summary`，新格式没有 → `#[serde(default, skip_serializing)]` 兼容读取，写入不再产生 |
| 查询层 | 遇到旧格式事件（无 `role`/`tool_call_id`），正常降级展示 |

---

## 8. 分阶段实施

| 阶段 | 内容 | 交付价值 |
|---|---|---|
| **P1** | 扩展 `ProgressDelta`（加字段 + `ToolResult`）；改造 `update_mapper` | UI 可还原完整对话流（消息/工具/文件编辑） |
| **P2** | `AgentProgress` 加 `ts` + `seq`；`journal.rs` 注入序号 | UI 时间线 + 因果排序 |
| **P3** | 查询层聚合 API（conversation/overview/tree） | UI 直接消费，无需自行重组事件 |
| **P4** | 可选：`AcpRaw` 选择性落盘 / plan 事件结构化 | 最完整的可观测性（plan 步骤、原始 payload） |

---

## 9. 测试计划

- **update_mapper 单测**：每个 `SessionUpdate` 分支验证新字段正确提取（`role`/`tool_call_id`/`input`/`op`/`diff`）
- **serde 兼容测试**：旧格式 JSONL 片段能反序列化，新字段取默认值
- **聚合查询测试**：构造模拟事件序列，验证 conversation/overview/tree 正确重组
- **端到端验证**：跑一次真实 agent run，检查 `events.jsonl` 中新字段完整性

---

## 10. 留待后续

- **Plan 事件结构化**：ACP 的 `SessionUpdate::Plan` 目前被丢弃（只有 raw），考虑加 `ProgressDelta::Plan` 捕获 agent 的计划步骤
- **消息去重**：流式 chunk 的 `message_id` 可用于 UI 端 chunk 组装，但需考虑同一消息的多个 chunk 在 JSONL 中的存储策略（逐 chunk 存储 vs 聚合后存储）
- **大 payload 截断**：工具 input/output 可能很大（如读整个文件），考虑配置最大 payload size，超出部分存路径引用
- **数据库迁移**：当 JSONL 文件增长到影响查询性能时（万级事件以上），考虑迁移到 SQLite 作为查询索引层
