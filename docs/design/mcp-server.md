# MCP Server — Workflow 编写指南与执行能力暴露

> **状态**: 📝 设计阶段（2025-08-19）
> **目标**: 新增 `luft-mcp` crate，作为外部 MCP server 供 AI 工具（Claude Code / Cline 等）调用，提供 workflow 编写指南（resources）和执行/查询能力（tools）。

---

## 1. 背景

Luft 当前有两套 MCP 实现，分别服务不同场景：

| 实现 | 位置 | 用途 | 协议方向 |
|------|------|------|---------|
| `mcp_server.rs` | `luft-cli/src/commands/` | structured output 验证（opencode 子进程） | agent → luft |
| `mcp.rs` | `luft/src/` | agent 上报 findings/artifacts/logs | agent → luft |

两者都是 **数据面**（agent 向 luft 上报），缺少 **控制面**（外部工具向 luft 下发指令）。

**本设计填补这个空缺**：一个面向外部 MCP client 的 server，让 AI 工具能够：
1. 通过 **resources** 学习如何编写 workflow Lua 脚本
2. 通过 **tools** 执行 workflow、查询状态、校验脚本

---

## 2. 架构

```
┌─────────────────────────────────────────────────────────┐
│  External MCP Client (Claude Code / Cline / ...)        │
│  ↕ stdio JSON-RPC                                       │
│                                                         │
│  $ luft mcp serve                                    │
│  ┌───────────────────────────────────────────────────┐  │
│  │ luft-mcp crate                                 │  │
│  │                                                   │  │
│  │  Resources (只读):                                │  │
│  │    workflow://schema       → Lua DSL 完整参考     │  │
│  │    workflow://examples     → 示例列表             │  │
│  │    workflow://example/{n}  → 单个示例内容         │  │
│  │                                                   │  │
│  │  Tools (可调用):                                  │  │
│  │    execute_workflow   → 校验+异步启动，返回 run_id │  │
│  │    list_workflows     → 列出可用 workflow 文件    │  │
│  │    get_run_status     → 查询 run 进度/状态        │  │
│  │    get_run_events     → 查询 run 事件流           │  │
│  │                                                   │  │
│  │  内部调用 luft facade / service / runtime      │  │
│  └───────────────────────────────────────────────────┘  │
│                                                         │
│  ── Workflow 执行时（ACP 桥接） ──────────────────────  │
│                                                         │
│  workflow.lua 的 agent() 调用:                          │
│    → ACP NewSessionRequest                             │
│    → mcp_servers: [{                                   │
│        command: "luft",                             │
│        args: ["mcp", "structured-output",              │
│               "--schema-file", <temp_path>]            │
│      }]                                                │
│    → opencode 子进程连接 MCP, 调用                      │
│      structured_output tool 提交 structured result     │
│                                                         │
│  （此部分与现有 mcp_server.rs 一致，不在本设计范围内）  │
└─────────────────────────────────────────────────────────┘
```

**与现有 MCP 实现的关系**：

| | 现有 `mcp.rs` / `mcp_server.rs` | 本设计 `luft-mcp` |
|---|---|---|
| 方向 | agent → luft（数据面上报） | client → luft（控制面指令） |
| 消费者 | workflow 内部的 agent 子进程 | 外部 AI 工具 |
| 启动方式 | luft 内部自动拉起 | `luft mcp serve` 手动启动 |
| 代码共享 | 不共享，独立实现 | — |

---

## 3. Crate 结构

### 3.1 目录

```
crates/luft-mcp/
├── Cargo.toml
└── src/
    ├── lib.rs          # pub API: run_mcp_server()
    ├── protocol.rs     # JSON-RPC types (McpRequest / McpResponse / McpError)
    ├── resources.rs    # resource handlers (schema / examples / example)
    ├── tools.rs        # tool handlers (execute / list / status / events / validate)
    └── server.rs       # stdio 主循环 + 方法分发
```

### 3.2 依赖关系

```toml
# crates/luft-mcp/Cargo.toml
[dependencies]
luft        = { path = "../luft" }          # facade API (run_script / run_workflow)
luft-service   = { path = "../luft-service" }   # query (status / events)
luft-runtime   = { path = "../luft-runtime" }   # validate_workflow
luft-core      = { path = "../luft-core" }      # types (RunId / RunOutcome / ...)
serde          = { version = "1", features = ["derive"] }
serde_json     = "1"
anyhow         = "1"
tokio          = { version = "1", features = ["rt", "rt-multi-thread", "sync"] }
```

```
luft-cli ──► luft-mcp ──► luft (facade)
                              ──► luft-service
                              ──► luft-runtime
                              ──► luft-core
```

### 3.3 CLI 集成

```rust
// crates/luft-cli/src/main.rs
#[derive(clap::Subcommand)]
enum Command {
    // ... existing commands ...

    /// MCP server operations
    #[command(subcommand)]
    Mcp(McpCommand),
}

#[derive(clap::Subcommand)]
enum McpCommand {
    /// Start MCP server (stdio JSON-RPC) for external AI tools
    Serve,
    /// Existing structured-output server (for ACP subprocess)
    StructuredOutput(McpStructuredOutputArgs),
}
```

---

## 4. Resources 设计

### 4.1 URI 方案

| URI | mimeType | 内容 |
|-----|----------|------|
| `workflow://schema` | `text/markdown` | Lua DSL 完整参考（`lua_dsl_reference.md`） |
| `workflow://examples` | `application/json` | 示例 workflow 列表（URI + 名称 + 描述） |
| `workflow://example/{name}` | `text/x-lua` | 单个示例 `.lua` 文件内容 |

### 4.2 `workflow://schema`

内容来源：编译时 `include_str!("../../luft-planner/src/lua_dsl_reference.md")` 嵌入二进制。

理由：该文件是 planner 生成 Lua 时使用的 system prompt，是 workflow DSL 最权威的参考。编译时嵌入避免运行时路径查找问题。

### 4.3 `workflow://examples`

动态扫描 `examples/` 和 `workflows/` 目录，返回 URI list：

```json
{
  "examples": [
    { "uri": "workflow://example/hello", "name": "hello", "description": "最简 workflow" },
    { "uri": "workflow://example/parallel", "name": "parallel", "description": "并行 fan-out" },
    ...
  ]
}
```

`description` 从每个 `.lua` 文件的 `meta.reasoning` 字段提取（使用 `luft-planner` 的 `extract_meta`）。

### 4.4 `workflow://example/{name}`

读取 `examples/{name}.lua` 或 `workflows/{name}.lua`，返回原始 Lua 源码。

### 4.5 MCP 协议响应

`resources/list`:
```json
{
  "resources": [
    {
      "uri": "workflow://schema",
      "name": "Workflow DSL Reference",
      "mimeType": "text/markdown",
      "description": "Complete Lua DSL syntax for writing Luft workflows"
    },
    {
      "uri": "workflow://examples",
      "name": "Example Workflows",
      "mimeType": "application/json",
      "description": "List of available example workflows"
    }
  ]
}
```

`resources/read`:
```json
{
  "contents": [
    {
      "uri": "workflow://schema",
      "mimeType": "text/markdown",
      "text": "## Workflow DSL Reference\n\n..."
    }
  ]
}
```

动态 resource template（`workflow://example/{name}`）通过 `resources/templates/list` 暴露：
```json
{
  "resourceTemplates": [
    {
      "uriTemplate": "workflow://example/{name}",
      "name": "Example Workflow",
      "description": "Read a specific example workflow by name",
      "mimeType": "text/x-lua"
    }
  ]
}
```

---

## 5. Tools 设计

### 5.1 `execute_workflow` — 执行 workflow

```json
{
  "name": "execute_workflow",
  "description": "Execute a Luft workflow. Accepts either inline Lua script or a path to a .lua file. Returns immediately with a run_id — use get_run_status to poll progress.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "script": { "type": "string", "description": "Inline Lua workflow script" },
      "path": { "type": "string", "description": "Path to .lua file (relative to CWD)" },
      "args": { "type": "object", "description": "Workflow arguments, accessible as `args` in Lua" }
    }
  }
}
```

**执行模式**：Fire & forget — 立即返回 `run_id`，不阻塞等待完成。

**前置校验**：执行前先调用 `luft_runtime::sandbox::validate_workflow(&script)` 进行三层校验（语法 → 结构 → schema）。校验失败则不执行，直接返回错误：

```json
{
  "content": [{
    "type": "text",
    "text": "{\"valid\":false,\"errors\":[\"missing meta table\",\"main function not defined\"]}"
  }],
  "isError": true
}
```

**返回**（校验通过后启动执行）：
```json
{
  "content": [{
    "type": "text",
    "text": "{\"run_id\": \"550e8400-e29b-41d4-a716-446655440000\", \"status\": \"running\"}"
  }]
}
```

**实现**：先 `validate_workflow`，通过后调用 `luft::Luft::run_script(script)` 或 `run_workflow(path)`，在 tokio runtime 的 `spawn` 中异步执行，主线程立即返回。

### 5.2 `list_workflows` — 列出可用 workflow

```json
{
  "name": "list_workflows",
  "description": "List available workflow files from workflows/ and examples/ directories",
  "inputSchema": { "type": "object", "properties": {} }
}
```

**返回**：
```json
{
  "content": [{
    "type": "text",
    "text": "[{\"name\":\"hello\",\"path\":\"examples/hello.lua\",\"description\":\"...\"}]"
  }]
}
```

### 5.3 `get_run_status` — 查询 run 状态

```json
{
  "name": "get_run_status",
  "description": "Get the current status of a workflow run",
  "inputSchema": {
    "type": "object",
    "properties": {
      "run_id": { "type": "string" }
    },
    "required": ["run_id"]
  }
}
```

**返回**（通过 `luft-service` query 模块 + `PhasesView`）：

MCP tool 返回的 `content[0].text` 是一个 JSON 字符串，解析后结构如下：

```jsonc
{
  // ── run 级状态 ──
  "run_id": "550e8400-e29b-41d4-a716-446655440000",
  "task": "Analyze codebase for security issues",
  "status": "running",           // CheckpointStatus 序列化：running | completed | failed | cancelled
  "current_phase": 2,            // 当前 phase 编号（从 1 开始）
  "total_phases": 5,
  "total_tokens": 12850,
  "elapsed_secs": 42.3,          // null 如果尚未结束
  "created_at": 1724044800,      // Unix timestamp

  // ── agent 统计 ──
  "completed_agents": 3,
  "running_agents": 1,
  "total_started": 4,

  // ── phase 详情 ──
  "phases": [
    {
      "phase_id": 1,
      "label": "Reconnaissance",
      "detail": "Scan codebase structure",    // null 如果没有 description
      "status": "completed",                   // pending | running | completed | failed
      "planned": 2,                            // 计划 agent 数（null 如果 dynamic）
      "ok": 2,                                 // 成功 agent 数
      "failed": 0,                             // 失败 agent 数
      "elapsed_secs": 12.1,
      "agents": [
        {
          "short_id": "a1b2c3d",               // AgentId 前 7 位
          "status": "completed",               // derived from CheckpointStatus
          "tokens": 4200,                      // null 如果未完成
          "findings": 3,                       // finding 数量
          "tool_count": 8,                     // null 如果后端不支持
          "last_message": "Found 3 potential..."  // null 如果无消息
        }
      ]
    },
    {
      "phase_id": 2,
      "label": "Deep Analysis",
      "detail": null,
      "status": "running",
      "planned": 3,
      "ok": 1,
      "failed": 0,
      "elapsed_secs": null,
      "agents": [
        {
          "short_id": "e4f5g6h",
          "status": "running",
          "tokens": null,
          "findings": 0,
          "tool_count": null,
          "last_message": null
        }
      ]
    }
  ],

  // ── 最终结果（仅 status=completed|failed 时存在）──
  "report": null,                             // report() 的 JSON 输出，失败时为 null
  "error": null                               // ScriptError 信息，仅 failed 时存在
}
```

**类型来源映射**：

| JSON 字段 | Rust 类型 | 来源 |
|-----------|-----------|------|
| `status` | `CheckpointStatus` (`state.rs`) | `RunCheckpoint.status` |
| `current_phase` / `total_phases` | `u32` | `RunCheckpoint.current_phase` / `PhasesView.run.total_phases` |
| `total_tokens` | `u64` | `RunCheckpoint.total_tokens` |
| `phases[].status` | `PhaseStatus` (`phases.rs`) | `PhasesView.phases[].status` |
| `phases[].agents[]` | `AgentRow` (`phases.rs`) | `PhasesView.phases[].agents[]` |
| `report` | `serde_json::Value` | `RunOutcome.result` (Ok) |
| `error` | `ScriptError` 信息 | `RunOutcome.result` (Err) |

**边界情况**：
- run_id 不存在 → 返回 `isError: true`，text 为 `{"error": "run not found: {run_id}"}`
- run 正在运行 → `report` 和 `error` 为 `null`
- run 被取消 → `status: "cancelled"`，`report: null`，`error` 含取消原因

### 5.4 `get_run_events` — 查询事件流

```json
{
  "name": "get_run_events",
  "description": "Get events for a workflow run, optionally only those after a specific event ID",
  "inputSchema": {
    "type": "object",
    "properties": {
      "run_id": { "type": "string" },
      "since_event_id": { "type": "string", "description": "Only return events after this event ID (for incremental polling)" }
    },
    "required": ["run_id"]
  }
}
```

**返回**：
```json
{
  "content": [{
    "type": "text",
    "text": "[{\"event_id\":\"evt-001\",\"type\":\"AgentStarted\",\"agent\":\"analyzer\",\"ts\":1234567890}, ...]"
  }]
}
```

---

## 6. Server 主循环

### 6.1 同步主循环 + tokio runtime

```rust
// crates/luft-mcp/src/server.rs

pub fn run_mcp_server(luft: Luft) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    // 共享状态：run_id → RunHandle
    let runs: Arc<Mutex<HashMap<RunId, RunHandle>>> = Arc::new(Mutex::new(HashMap::new()));

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() { continue; }

        let msg: JsonRpcMessage = serde_json::from_str(&line)?;
        let id = msg.id.clone();

        match msg.method.as_deref() {
            "initialize"              => { /* capabilities: tools + resources */ }
            "notifications/initialized" => { /* noop */ }
            "ping"                    => { /* {} */ }
            "resources/list"          => { /* 返回静态 resource 列表 */ }
            "resources/templates/list"=> { /* 返回 example/{name} template */ }
            "resources/read"          => { /* 分发到 resources.rs */ }
            "tools/list"              => { /* 返回 4 个 tool 定义 */ }
            "tools/call"              => {
                // 在 rt 上 block_on 执行 async tool handler
                let result = rt.handle().block_on(
                    tools::handle_call(&msg.params, &luft, &runs)
                );
            }
            _ => { /* -32601 Method not found */ }
        }
    }
    Ok(())
}
```

### 6.2 异步 tool 执行

`execute_workflow` 内部：

```rust
async fn execute_workflow(luft: &Luft, script: &str, args: Value) -> Result<Value> {
    let handle = luft.run_script(script).await?;  // 非阻塞 spawn
    let run_id = handle.run_id();
    // 不 await handle，立即返回 run_id
    Ok(json!({ "run_id": run_id.to_string(), "status": "running" }))
}
```

`get_run_status` / `get_run_events` 通过 `luft-service` 的 query 模块查询，从 `RunHandle` 或 storage reader 获取。

### 6.3 MCP capabilities 声明

```json
{
  "protocolVersion": "2024-11-05",
  "capabilities": {
    "tools": {},
    "resources": {}
  },
  "serverInfo": {
    "name": "luft",
    "version": "0.1.0"
  }
}
```

---

## 7. 设计决策与权衡

### 7.1 同步主循环 + block_on vs 全异步

| | 同步主循环 + `rt.handle().block_on()` | 全异步（tokio::io） |
|---|---|---|
| 测试 | 容易（fd redirection / pipe 模拟） | 需要 async test harness |
| 代码复杂度 | 低 | 中 |
| 与现有实现一致性 | 与 `mcp_server.rs` 一致 | 与 `mcp.rs` 一致 |

**选择**：同步主循环。MCP 协议是请求/响应模型，无并发请求，同步足够。

### 7.2 Fire & forget vs 阻塞等待

| | Fire & forget | 阻塞等待完成 |
|---|---|---|
| 客户端体验 | 需要轮询 `get_run_status` | tool call 可能耗时数分钟 |
| MCP 兼容性 | 所有 client 都支持 | 部分 client 有超时限制 |
| 扩展性 | 天然支持多 run 并发 | 串行 |

**选择**：Fire & forget。Workflow 执行时间不可预测（分钟级），阻塞会导致 client 超时。

### 7.3 Resource 内容：编译时嵌入 vs 运行时读取

| | `include_str!` 编译时嵌入 | 运行时文件读取 |
|---|---|---|
| 部署 | 单二进制，无路径依赖 | 需要保证文件存在 |
| 更新 | 需要重新编译 | 即时生效 |
| 适用性 | 静态参考文档 | 动态内容 |

**选择**：`workflow://schema` 用 `include_str!`（静态、权威），`workflow://examples` 和 `workflow://example/{name}` 用运行时文件读取（动态内容）。

### 7.4 `examples/` 目录定位

运行时如何定位 `examples/` 目录？

**方案**：按优先级查找：
1. `luft` 的 `base_dir` 配置（如果设置了）
2. 当前工作目录 `CWD/examples/`
3. 相对于可执行文件路径

---

## 8. 实现计划

### Phase 1: 骨架 + Resources
- [ ] 创建 `crates/luft-mcp/` crate
- [ ] 实现 `protocol.rs`（JSON-RPC types）
- [ ] 实现 `server.rs`（stdio 主循环）
- [ ] 实现 `resources.rs`（schema + examples + example）
- [ ] CLI 集成 `luft mcp serve`

### Phase 2: Tools
- [ ] 实现 `tools.rs`（4 个 tool handlers）
- [ ] `execute_workflow` 前置 `validate_workflow` + 接入 `luft::Luft` facade
- [ ] `get_run_status` / `get_run_events` 接入 `luft-service` query

### Phase 3: 测试
- [ ] 单元测试（protocol / resource URI 解析 / tool handler 逻辑）
- [ ] 集成测试（stdio pipe 模拟完整 MCP 会话）
- [ ] 端到端测试（真实 MCP client 对接）

---

## 9. 开放问题

| # | 问题 | 当前倾向 | 待确认 |
|---|------|---------|--------|
| 1 | `Luft` facade 是否已提供异步非阻塞的 `run_script` 接口？ | 需验证 `RunHandle` 是否 spawn 后立即返回 | 查看 facade API |
| 2 | `luft-service` 的 query 模块是否支持按 `run_id` 查询状态和事件？ | 需验证 query API 完整性 | 查看 service API |
| 3 | 多 run 并发时 `RunHandle` 的生命周期管理 | `Arc<Mutex<HashMap<RunId, RunHandle>>>` | 确认 `RunHandle` 是否需要手动 drop |
