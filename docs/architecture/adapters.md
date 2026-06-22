# adapters 模块架构

> **OpenCode ACP 后端（P0-A）。** 把 `opencode acp` 子进程驱动为一个 `AgentBackend`：作为 ACP **客户端**完成一次性会话，将流式更新映射成 Maestro 进度事件，最后收集结构化结果。

源码：[`src/adapters/`](../../src/adapters/) ｜ 公开 API：[`src/adapters/mod.rs`](../../src/adapters/mod.rs)

---

## 1. 职责与边界

`adapters` 是 `core` 的 `AgentBackend` trait 的**真实实现**（与之相对的是 core 里的 `MockBackend` 测试实现）。它把 Maestro 的 `AgentTask` 翻译成一次完整的 [ACP](https://agentclientprotocol.com)（Agent Client Protocol）会话，再把会话结果翻译回 `AgentResult`。

```
   Scheduler ──run(task, ctx)──► AcpAdapter ──spawn──► `opencode acp` 子进程
                                     │  (ACP client)        │
                                     │◄── session/update ───┤  通知流
                                     │── initialize ────────►│
                                     │── session/new ───────►│
                                     │── session/prompt ────►│
                                     │◄── stop_reason ───────┤
                                     ▼
                                 AgentResult
```

**边界**：adapters **只懂 ACP 与子进程**，不碰调度/持久化/Lua。它消费 `core` 的合约类型（`AgentTask`/`AgentResult`/`ToolPolicy`/`ProgressDelta`/`Finding`），并通过 `register_acp_backend()` 注册进 `BackendRegistry`。

---

## 2. 内部结构

| 文件 | 职责 |
|------|------|
| [`acp_adapter.rs`](../../src/adapters/acp_adapter.rs) | `AcpConfig` + `AcpAdapter`；一次性会话生命周期 `run_acp_session` |
| [`update_mapper.rs`](../../src/adapters/update_mapper.rs) | ACP `SessionUpdate` → Maestro `ProgressDelta`；累积 message 文本与 token |
| [`permission.rs`](../../src/adapters/permission.rs) | 非交互 `request_permission` 自动决策（纯逻辑 + 单测） |
| [`result_collector.rs`](../../src/adapters/result_collector.rs) | stop_reason + message → `AgentResult`（findings 文本回退解析） |

依赖第三方 crate [`agent-client-protocol`](https://crates.io/crates/agent-client-protocol) `0.11.1` 提供 ACP schema 与连接原语。

---

## 3. 线程模型：!Send 的连接未来如何接入 Send 的 trait（关键）

ACP 连接 future 驱动一个 `LocalSet`，因而是 **`!Send`** 的；但 `AgentBackend::run` 是 `#[async_trait]`，要求返回 `Send` future。桥接方式：

```rust
async fn run(&self, task, ctx) -> Result<AgentResult, BackendError> {
    tokio::task::spawn_blocking(move || {
        let rt = current_thread Runtime;          // 专属当前线程运行时
        let local = LocalSet::new();
        local.block_on(&rt, run_acp_session(...))  // 在 LocalSet 内驱动 !Send future
    }).await?
}
```

整个会话跑在 `spawn_blocking` 出来的独立线程上的 current-thread runtime + LocalSet 里，最终把 `Send` 的 `AgentResult` 交还给共享 worker 池。这与 [runtime](./runtime.md) 的"SDK 在阻塞线程 block_on"是两个独立的阻塞边界，各自隔离。

---

## 4. 一次性会话生命周期（run_acp_session）

```
① spawn `opencode acp`               stdin/stdout=piped, stderr=null
② ByteStreams transport              compat 适配 tokio ↔ futures-io
③ 构建 ACP Client，挂两个回调:
     on_receive_notification ─► update_mapper::handle_update(累积+emit 进度)
     on_receive_request      ─► permission::decide(自动批准/拒绝)
   connect_with 内按序请求:
     initialize(ProtocolVersion::V1)
     session/new(cwd)        cwd = canonicalize(task.workdir)
     session/prompt(text)    ← task.prompt
     捕获 stop_reason
④ tokio::select! 三方竞速:
     会话完成  / ctx.cancel 触发(→kill 子进程, Cancelled) / 超时(默认 300s, →kill, Timeout)
⑤ result_collector::collect(task, stop, accumulated_message, tokens)
```

连接关闭类错误（"receiver dropped"/"broken pipe"/"unexpected eof"/"connection closed"）被归一化为 `BackendError::Protocol("connection closed")`。

`AcpConfig`：`binary`（默认 `opencode`，从 PATH 解析）、`log_level`、`connect_timeout`（默认 10s）。`capabilities()` 声明 `streaming=true`，`mcp_injection=false`（MCP 数据面注入是 P1）、`structured_output=false`。

---

## 5. 子模块细节

### 5.1 update_mapper —— 流式更新 → 进度事件

`Accumulator{ message, tokens }`（均 `Mutex`）在会话期间累积。`handle_update` 按 `SessionUpdate` 变体分发：

| ACP 更新 | 动作 | 产出 `ProgressDelta` |
|----------|------|---------------------|
| `AgentMessageChunk` | 累积进 `message` | `Message{text}` |
| `AgentThoughtChunk` | 不累积 | `Message{"[reasoning] …"}` |
| `ToolCall` | — | `ToolCall{name,summary}` |
| `ToolCallUpdate` | — | `FileEdit{path}`（若含 path） |
| 任意带 `usage` 对象 | 更新 token 总量 | `Tokens{usage}` |

> ACP schema 类型是宏生成的，嵌套字段类型不稳定，因此 mapper 走**序列化成 JSON 再递归挖字段**（`find_str`/`find_object`）的启发式路径——只对顶层 `SessionUpdate` 变体名做精确匹配。`usage` 字段兼容 `input_tokens`/`input` 等多种命名。

### 5.2 permission —— 非交互决策

v0.1 **永不阻塞等人**：每个 `session/request_permission` 都由 `decide(policy, inputs)` 同步裁决。该函数是**纯函数**、有单测；`extract_inputs` 负责把 ACP 请求挖成纯输入。

决策优先级：

```
无 policy            → Approve（让自洽 agent 自由工作）
有 policy:
   deny 命中(命令含子串)   → Deny
   文件编辑              → accept_edits ? Approve : Deny
   shell 命令            → allow_commands 前缀匹配 ? Approve : Deny
   MCP 工具              → allow_mcp 精确匹配 ? Approve : Deny
   其他                  → Approve（opencode 自管读类操作）
```

批准时 handler 选择请求里的**第一个** option；拒绝则返回 `Cancelled`。

### 5.3 result_collector —— 组装 AgentResult

```
stop_reason(Debug 串) ── status_from_stop_reason ──►
     含 "EndTurn" → Ok ｜ 含 "Cancel" → Cancelled ｜ 其余(MaxTokens/Refused/…) → Error
message ── extract_findings_from_output ──►
     从原始 JSON 或 ``` 围栏块里解析 {findings:[...]} / {finding:...}
output = findings 非空 ? findings 的 JSON : { "text": message }
```

> v0.1 走的是**消息回退路径**：findings 从 agent 最终文本里解析。结构化的 MCP `report_finding` 集成（让 agent 主动上报）是 P1——见 [mcp.md](./mcp.md)。stop_reason 用 `contains` 松匹配，避免依赖宏生成的精确变体名。

---

## 6. 设计决策与权衡

- **一次 run = 一次性会话**：每个 agent 任务独立 spawn/initialize/prompt，无会话复用——简单、隔离好，代价是进程启动开销。
- **启发式 JSON 挖字段而非强类型绑定**：ACP schema 宏生成、版本易变，挖字段让 adapter 对 schema 微调更鲁棒，代价是字段名假设是隐式的。
- **决策逻辑与 ACP 解耦**：`decide` 纯函数化，使权限策略可单测、可独立演进，不被 ACP 请求形状绑架。
- **取消/超时直接 kill 子进程**：保证不留僵尸 opencode 进程，代价是丢弃部分进行中的工作。

---

## 7. 当前状态与局限（v0.1）

- 仅实现 `opencode` 一种后端；多后端/能力路由是 v0.2。
- `mcp_injection=false`：agent 当前**不连**到 Maestro 的 MCP 数据面（见 [mcp.md](./mcp.md)），findings 只能从文本回退解析。
- `extract_inputs` 的 `mcp_tool` 恒为 `None`——MCP 工具维度的权限尚未接线。
- token 用量依赖 opencode 在 update 里上报 `usage`；若后端不报则为 0。

---

## 8. 相关文档

- 总览：[../architecture.md](../architecture.md)
- 依赖：core.md（`AgentBackend` trait、`AgentTask`/`AgentResult`/`ToolPolicy`/`ProgressDelta`）
- 协作：[mcp.md](./mcp.md)（P1 结构化上报路径）、[runtime.md](./runtime.md)（converge 如何消费 findings）
- 旧版设计稿：backends.md（已归档）、[../design/p0-acp-backend.md](../design/p0-acp-backend.md)
