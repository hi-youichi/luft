# P0-A: OpenCode ACP 真实后端 — 实现设计

> **路线图引用**: `roadmap.md` §P0-A
> **状态**: ✅ 已实现（2026-06-03，live 验证通过）
> **交叉参考**: `backends.md（已归档）` — 原始后端设计（部分已过时，以本文档为准）

> **实现与本设计的偏差（以代码为准）**：实际实现比下文的 7 模块方案精简。
> 关键发现：① `agent-client-protocol` 0.11.1 的 `Client.builder().connect_with(transport, |conn| async {…})`
> 高级 API 把整个一次性会话（initialize → session/new → session/prompt）封装在闭包里，无需设计文档
> §4.2 的 oneshot 桥接；② opencode 自带 fs/terminal，client 只需注册 `notification` + `request_permission`
> 两个 handler（fs/terminal handler 省略）；③ ACP 连接 future 是 `!Send`，而 `AgentBackend::run` 是
> `#[async_trait]`（Send），用 **current-thread runtime + LocalSet 在 `spawn_blocking` 内驱动**、回传 Send
> 结果来桥接。最终模块：`acp_adapter` / `update_mapper` / `permission` / `result_collector` / `mod`。

---

## 1. 现状问题

`src/adapters.rs`（443 行）五种核心缺陷：

| # | 问题 | 影响 |
|---|------|------|
| 1 | `AcpConfig` 字段散乱（`backend_type`/`executable`/`ws_url`/`mcp_endpoint`…），与设计目标 `{ binary, log_level, connect_timeout }` 不一致 | 配置混乱 |
| 2 | 手拼 JSON-RPC，未使用 `agent-client-protocol` crate | 协议不可靠、难维护 |
| 3 | `run()` 传参 `_ctx: RunContext`（已忽略），丢失 `CancellationToken` + `EventSender` | 无法取消、无进度上报 |
| 4 | 无进度上报 — `AgentProgress` 事件永远不产生 | 不可观测 |
| 5 | 无取消路径 — `ctx.cancel` 被忽略 | 任务挂死 |

---

## 2. 目标架构

```
src/adapters/
├── mod.rs               # pub use AcpAdapter; register "opencode" backend
├── acp_adapter.rs       # AcpAdapter (impl AgentBackend) + AcpConfig
├── acp_session.rs       # AcpSession: 子进程 + ConnectionTo<Agent> + 生命周期
├── client_handler.rs    # Client 侧回调: fs/read·write, terminal/*
├── permission.rs        # ToolPolicy → auto_decide 权限逻辑
├── update_mapper.rs     # SessionUpdate → ProgressDelta 映射
└── result_collector.rs  # MCP findings 优先 / 最终消息回退
```

---

## 3. 新增依赖

```toml
# Cargo.toml [dependencies] 新增
agent-client-protocol = "0.11"
agent-client-protocol-schema = "0.12"
shell-words = "1"       # 解析命令字符串（ACP example 同款）

# tokio-util 已有，需确认 features 含 "compat"
```

---

## 4. 模块详细设计

### 4.1 acp_adapter.rs — 入口

```rust
use std::path::PathBuf;
use std::time::Duration;

/// ACP 后端配置。精简为三个字段。
#[derive(Clone)]
pub struct AcpConfig {
    pub binary: PathBuf,           // 默认 "opencode"（从 PATH 查找）
    pub log_level: Option<String>, // 传给 opencode --log-level
    pub connect_timeout: Duration, // initialize 握手超时，默认 10s
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("opencode"),
            log_level: None,
            connect_timeout: Duration::from_secs(10),
        }
    }
}

pub struct AcpAdapter { config: AcpConfig }

impl AcpAdapter {
    pub fn new(config: AcpConfig) -> Self { Self { config } }
    pub fn default_opencode() -> Self { Self::new(AcpConfig::default()) }
}

#[async_trait]
impl AgentBackend for AcpAdapter {
    fn id(&self) -> &'static str { "opencode" }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            mcp_injection: true,
            structured_output: false,  // v0.1: 依赖 MCP report_finding
            models: vec![],            // opencode 自管理
        }
    }

    async fn run(&self, task: AgentTask, ctx: RunContext) -> Result<AgentResult, BackendError> {
        let mut session = self.spawn_and_initialize(&task, &ctx).await?;

        let stop_reason = tokio::select! {
            r = session.prompt(&task.prompt) => r?,
            _ = ctx.cancel.cancelled() => {
                session.cancel().await.ok();
                return Err(BackendError::Cancelled);
            }
            _ = tokio::time::sleep(task.timeout.unwrap_or(DEFAULT_TIMEOUT)) => {
                session.cancel().await.ok();
                return Err(BackendError::Timeout);
            }
        };

        result_collector::collect(session, stop_reason, &task).await
    }
}
```

### 4.2 acp_session.rs — ACP 连接生命周期

```rust
use agent_client_protocol::{Client, Agent, ConnectionTo, ByteStreams};
use agent_client_protocol::schema::{InitializeRequest, ProtocolVersion, SessionId};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

pub struct AcpSession {
    pub child: tokio::process::Child,
    pub conn: ConnectionTo<Agent>,
    pub session_id: SessionId,
    // 共享状态，供 update_mapper / result_collector 读写
    pub last_agent_message: Arc<Mutex<String>>,
    pub token_accum: Arc<Mutex<TokenUsage>>,
    // oneshot sender — drop 时通知 connect_with 闭包退出
    done_tx: Option<tokio::sync::oneshot::Sender<()>>,
}
```

**ACP 连接模式（关键难点 D1）**:

ACP `connect_with` 要求闭包持有 `ConnectionTo<Agent>` 的所有权并在闭包内完成全部操作。但 Maestro 需要在 `AcpAdapter::run` 中外部持有连接来发 `session/prompt`。

解决方案：**oneshot channel 桥接**。

```
spawn_and_initialize:
  connect_with(transport, |conn| {
      tx.send(conn)?;        // 传出 ConnectionTo<Agent>
      rx.await               // 阻塞，直到 AcpSession drop（done_tx drop → rx 返回）
  })

  // 外部通过 rx.await 拿到 ConnectionTo<Agent>，
  // 后续在外部发 session/new + session/prompt

AcpSession::drop:
  drop(done_tx)  → rx 返回 → connect_with 闭包退出 → 连接关闭
```

**完整 spawn 流程**:

1. `Command::new("opencode").arg("acp")` spawn 子进程（stdin/stdout piped）
2. `ByteStreams::new(stdin.compat_write(), stdout.compat())` 构造 transport
3. `Client.builder()`
   - `.on_receive_notification(update_callback)` — 映射 SessionUpdate → ProgressDelta
   - `.on_receive_request(permission_callback)` — fs/terminal 反向回调
   - `.connect_with(transport, main_fn)` — 在 main_fn 中用 oneshot 传出 ConnectionTo
4. 外部拿到 `ConnectionTo<Agent>` 后：`send_request(InitializeRequest)` → `send_request(NewSessionRequest)`
5. 返回 `AcpSession`

**Prompt 流程**:

```rust
impl AcpSession {
    pub async fn prompt(&self, text: &str) -> Result<StopReason, BackendError> {
        let content = vec![ContentBlock::Text(TextContent::new(text.to_string()))];
        let request = PromptRequest::new(self.session_id.clone(), content);
        let response = self.conn.send_request(request)
            .block_task().await
            .map_err(|e| BackendError::Protocol(e.to_string()))?;
        Ok(response.stop_reason)
    }
}
```

**时序图**:

```
Maestro(AcpAdapter)                    opencode acp (子进程)
       |                                      |
       |-- spawn opencode acp -------------->>|
       |== ByteStreams ======================|
       |-- initialize --------------------->>|
       |<<-- InitializeResponse ------------|
       |-- session/new {cwd, mcpServers} -->>|
       |<<-- NewSessionResponse ------------|
       |-- session/prompt {content} ------->>|
       |<<-- session/update(MessageChunk) --|  → AgentProgress(Message)
       |<<-- session/update(ToolCall) ------|  → AgentProgress(ToolCall)
       |-- [fs/read_text_file 反向请求] <<---|
       |-- [response: file content] ------>>|
       |<<-- session/update(Tokens) --------|  → AgentProgress(Tokens)
       |<<-- PromptResponse {EndTurn} ------|
```

### 4.3 update_mapper.rs — SessionUpdate → ProgressDelta

| ACP SessionUpdate 变体 | → ProgressDelta | 处理 |
|---|---|---|
| `AgentMessageChunk { text, .. }` | `Message { text }` | 拼接到 `last_agent_message` |
| `AgentThoughtChunk { text, .. }` | `Message { text }` | 加 `[reasoning]` 前缀 |
| `ToolCall { title, kind, status:Pending }` | `ToolCall { name: title, summary: kind }` | 工具开始 |
| `ToolCallUpdate { status:Completed, Diff{path} }` | `FileEdit { path }` | 文件变更 |
| `ToolCallUpdate { status:Completed/Failed, content }` | `ToolCall { name, summary }` | 工具结束 |
| `SessionInfoUpdate { usage:{input,output} }` | `Tokens { usage }` | token 累加到 `token_accum` |
| `UserMessageChunk` | 忽略 | Maestro 自身发的 prompt echo |
| `Plan` / `CurrentModeUpdate` | 忽略 | v0.1 不处理 |

每个 `ProgressDelta` 包装为 `AgentEvent::AgentProgress { run_id, agent_id, delta }` 经 `ctx.events.send(...)` 上报。

```rust
pub struct MapperState {
    pub last_agent_message: Arc<Mutex<String>>,
    pub token_accum: Arc<Mutex<TokenUsage>>,
}

pub fn map_update(
    update: SessionNotification,
    run_id: RunId,
    agent_id: AgentId,
    state: &MapperState,
    events: &EventSender,
) {
    // 匹配 update.update（SessionUpdate 枚举），映射到 ProgressDelta，
    // 然后发送 AgentProgress 事件。
    // 同时更新 MapperState 的共享字段。
}
```

### 4.4 client_handler.rs — ACP Agent → Client 反向回调

OpenCode 作为 ACP Agent，执行过程中会**反向调用** Maestro（Client 侧）：

| ACP 反向请求 | Client 处理 |
|---|---|
| `fs/read_text_file` | `tokio::fs::read_to_string(workdir + path)`，支持 line/limit 分片 |
| `fs/write_text_file` | 查 `ToolPolicy.accept_edits`；通过则写，否则返回 permission_denied |
| `terminal/create` | 查 `allow_commands` 前缀匹配；通过则 `tokio::process::Command` 执行 |
| `terminal/output` / `wait_for_exit` / `kill` / `release` | 操作 `DashMap<TerminalId, JoinHandle>` |
| `request_permission` | 调 `permission::auto_decide` 后立即 respond（不 await 用户） |

```rust
pub struct ClientHandler {
    workdir: PathBuf,
    policy: Option<ToolPolicy>,
    terminals: DashMap<String, JoinHandle<()>>,
}
```

### 4.5 permission.rs — 权限自动决策

```rust
pub enum PermissionOutcome {
    Approve,
    Deny(String),
}

pub fn auto_decide(
    req: &RequestPermissionRequest,
    policy: Option<&ToolPolicy>,
) -> PermissionOutcome {
    let policy = match policy {
        None => return PermissionOutcome::Deny("no policy configured".into()),
        Some(p) => p,
    };
    // 优先级链：deny list → accept_edits → allow_commands → allow_mcp → 兜底 Deny
    if policy.deny.iter().any(|d| tool_matches(req, d)) {
        return PermissionOutcome::Deny("tool in deny list".into());
    }
    if is_file_edit_request(req) {
        return if policy.accept_edits { Approve } else { Deny("accept_edits=false".into()) };
    }
    if let Some(cmd) = extract_command(req) {
        return if policy.allow_commands.iter().any(|p| cmd.starts_with(p.as_str())) {
            Approve
        } else {
            Deny(format!("not in allowlist: {cmd}"))
        };
    }
    if let Some(tool) = extract_mcp_tool(req) {
        return if policy.allow_mcp.iter().any(|n| n == &tool) { Approve }
               else { Deny("mcp tool not allowed".into()) };
    }
    Deny("unknown tool type in non-interactive mode".into())
}
```

`request_permission` 回调中调用 `auto_decide` 后立即 `responder.respond(...)`。v0.1 全程非交互，**绝不 await 用户输入**。

### 4.6 result_collector.rs — 结果收集

```rust
pub async fn collect(
    session: AcpSession,
    stop_reason: StopReason,
    task: &AgentTask,
) -> Result<AgentResult, BackendError> {
    let status = match stop_reason {
        EndTurn => AgentStatus::Ok,
        Cancelled => AgentStatus::Cancelled,
        MaxTokens | MaxTurns | Refused => AgentStatus::Error,
    };

    // v0.1: 仅 message 回退路径（MCP report_finding 集成留 P1）
    let message = session.last_agent_message.lock().unwrap().clone();
    let findings = extract_findings_from_output(&message);
    let output = if !findings.is_empty() {
        serde_json::to_value(&findings).unwrap_or(serde_json::Value::Null)
    } else {
        parse_final_message(&message)
    };
    let tokens_used = *session.token_accum.lock().unwrap();

    Ok(AgentResult {
        agent_id: task.agent_id,
        status,
        output,
        findings,
        tokens_used,
        artifacts: vec![],
        logs: Default::default(),
    })
}
```

---

## 5. 关键设计决策

| # | 决策 | 选择 | 理由 |
|---|------|------|------|
| D1 | ACP 连接生命周期 | oneshot channel 传出 `ConnectionTo<Agent>` | ACP `connect_with` 要求闭包所有权，需用 channel 打破所有权限制 |
| D2 | 进程管理 | `tokio::process::Child` + `Drop::drop` start_kill | 与 ACP yolo_one_shot_client 同模式；Drop 兜底防止僵尸进程 |
| D3 | 权限模式 | 全自动批准（ToolPolicy 静态规则） | v0.1 非交互模式，不允许卡在权限请求上 |
| D4 | findings 来源 | 仅 message 回退路径 | MCP report_finding 集成留 P1；降低 P0 复杂度 |
| D5 | 错误重试 | 不改 Scheduler（已有指数退避） | AcpAdapter 只管单次 run，重试由上层处理 |

---

## 6. 错误处理矩阵

| 情形 | 处置 | BackendError | 可重试 |
|------|------|-------------|--------|
| spawn 失败 | 立即返回 | `Spawn(msg)` | ✅ |
| initialize 超时 | kill 子进程 | `Timeout` | ✅ |
| session/new 协议错 | kill 子进程 | `Protocol(msg)` | ❌ |
| ctx.cancel | session/cancel → kill | `Cancelled` | ❌ |
| wall clock 超时 | 同取消路径 | `Timeout` | ✅ |
| 连接断开（进程崩溃） | 检测 EOF | `Protocol("connection closed")` | ✅ |
| MaxTokens / Refused | 正常 collect | status=Error | ❌ |

---

## 7. 与 loom-acp 的借鉴点（仅参考，不复用代码）

| 借鉴点 | loom-acp 来源 | AcpAdapter 用法（方向相反） |
|---|---|---|
| `ByteStreams::new(stdin.compat, stdout.compat_write)` | `transport/core.rs` | 完全相同的 transport 构造 |
| `Agent.builder().on_receive_request(...)` | `transport/core.rs` | 对称用 `Client.builder()` |
| `ConnectionTo<_>` + `send_request` | `client/methods.rs` | Maestro 作为 Client 发请求 |
| `is_connection_closed_error` | `transport/core.rs` | update 循环用相同判断 |

---

## 8. 测试清单

### 单元测试（CI 必跑，不需真实 binary）

| 测试名 | 模块 | 验证点 |
|--------|------|--------|
| `test_acp_config_default` | acp_adapter | binary="opencode", timeout=10s |
| `test_update_mapper_agent_message_chunk` | update_mapper | AgentMessageChunk → Message |
| `test_update_mapper_tool_call` | update_mapper | ToolCall(Pending) → ToolCall delta |
| `test_update_mapper_tokens` | update_mapper | SessionInfoUpdate → Tokens delta |
| `test_permission_deny_list` | permission | deny=["rm"] + "rm -rf /" → Deny |
| `test_permission_accept_edits` | permission | accept_edits + 文件编辑 → Approve |
| `test_permission_allow_commands_prefix` | permission | allow_commands=["cargo"] + "cargo test" → Approve |
| `test_result_collector_message_fallback` | result_collector | findings 非空 → output 为 findings JSON |

### 集成测试（#[ignore]，需真实 opencode binary）

| 测试名 | 验证点 |
|--------|--------|
| `test_acp_full_round_trip` | 最小 prompt → status==Ok, output 非空 |
| `test_acp_cancel_mid_run` | 200ms 后 cancel → BackendError::Cancelled，不挂死 |
| `test_acp_timeout` | 短 timeout → BackendError::Timeout |
