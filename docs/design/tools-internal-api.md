# Internal Tools Design

> 将 MCP 服务器改为 Loom 内置工具（Tool trait），最小工具集合方案。

**Status**: Design / Discussion
**Author**: AI Agent
**Created**: 2025-01-16
**Stakeholders**: @apple

---

## 1. Background

Maestro 当前使用两套 MCP 服务器：

| 位置 | 方向 | 工具数量 | 传输方式 |
|------|------|---------|----------|
| `luft-mcp` crate | 外部 AI 客户端 → Luft (inbound) | 4 | stdio JSON-RPC server |
| `luft/src/mcp.rs` | Luft agent → Luft core (data-plane) | 5 | stdio JSON-RPC server |

两套都维护了自己的：
- JSON-RPC 协议层 (`JsonRpcMessage`, `JsonRpcResponse`, `JsonRpcError`)
- stdio 传输层（读 stdin 行、写 stdout 行）
- MCP initialize/resources/templates 协议
- 工具定义（硬编码的 JSON Schema）

这带来了额外的复杂度和维护成本。

### 目标

将这两套 MCP 工具改为**内置 Tool**（实现 `Tool` trait），消除协议层和传输层开销。

### 非目标

- 保留外部 AI 客户端（Claude Desktop 等）的访问能力 —— 不再需要 stdio server
- 保留 MCP 协议兼容性 —— 彻底删除 JSON-RPC 代码

---

## 2. Requirements

### 2.1 Functional Requirements

| Req | Description |
|-----|-------------|
| FR1 | 提供 **最小工具集合**（2 个工具）：`execute_workflow`, `get_run` |
| FR2 | `execute_workflow` 支持 fire-and-forget 模式，返回 `run_id` |
| FR3 | `get_run` 同时返回状态和事件，支持分页（`event_limit`）和增量查询（`since_event_id`） |
| FR4 | `get_run` 的 status 包含**运行中 agents 详细信息**（运行时长、工具调用次数、token 消耗） |
| FR5 | workflow 列表和 Lua DSL 参考内嵌到 system prompt，不再需要 MCP resources |
| FR6 | 工具由 Lua SDK 调用，作为新的 SDK primitive |

### 2.2 Non-Functional Requirements

| Req | Description |
|-----|-------------|
| NFR1 | Tool trait 瘦接口，依赖通过 struct 字段注入（非全局 ctx） |
| NFR2 | 共享状态通过 `Arc` 共享，零 clone 开销 |
| NFR3 | 代码删除干净，不留 JSON-RPC、stdio、MCP 协议残留 |
| NFR4 | 向后兼容旧的 checkpoint 格式（`ts` 字段 `#[serde(default)]`） |

---

## 3. Existing System

### 3.1 Maestro MCP 架构

```
┌──────────────────────────────────────────────────────────┐
│                   luft-mcp crate                        │
│  ┌───────────┐  ┌────────────┐  ┌──────────────────┐   │
│  │ protocol  │  │   tools    │  │    resources     │   │
│  │ (JSON-RPC)│  │  (4 tools) │  │  (schema/exam)   │   │
│  └───────────┘  └────────────┘  └──────────────────┘   │
│  ┌─────────────────────────────────────────────────┐   │
│  │              server.rs (stdio)                  │   │
│  └─────────────────────────────────────────────────┘   │
└──────────────┬──────────────────────────────────────────┘
               │ stdio (newline JSON-RPC)
┌──────────────▼──────────────────────────────────────────┐
│                     Luft                                │
│  ┌────────────┐  ┌────────────┐  ┌──────────────────┐  │
│  │   luft     │  │   runs     │  │  search_dirs     │  │
│  │  (facade)  │  │ (registry) │  │  (examples/work) │  │
│  └────────────┘  └────────────┘  └──────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

### 3.2 原有工具映射

#### Inbound 工具组（luft-mcp/tools.rs）

| 原工具 | 功能 | 输入 | 输出 |
|---|---|---|---|
| `execute_workflow` | 启动 workflow，注册 run_id | `script` \| `path`, `args?` | `run_id`, `run_dir`, `status: "running"` |
| `list_workflows` | 列出可用 .lua 文件 | 无 | `[ { name, path, description } ]` |
| `get_run_status` | 查询 run 状态 | `run_id` | `StatusOutput` |
| `get_run_events` | 获取事件流 | `run_id`, `since_event_id?` | `[ AgentEvent ]` |

#### Data-plane 工具组（luft/src/mcp.rs）

| 原工具 | 功能 | 输入 | 输出 |
|---|---|---|---|
| `report_finding` | 回报结构化发现 | `run_id`, `kind`, `title`, `detail` | success |
| `report_artifacts` | 回报产物 | `run_id`, `artifacts[]` | success |
| `report_log` | 回报日志 | `run_id`, `level`, `msg` | success |
| `report_status` | 回报进度 | `run_id`, `status`, `progress?` | success |
| `request_next_task` | 请求 converge 下一个任务 | `run_id` | `Task \| None` |

### 3.3 已完成的前置调整

**Maestro 事件系统增强**（commit `17ff921`）：

- `AgentStarted` 新增 `ts: DateTime<Utc>` 字段（`#[serde(default)]`）
- `AgentDone` 已有 `ts` 字段
- 更新所有事件发射点（`scheduler/mod.rs`、`journal.rs`）

这支持了运行时计算 agent 运行时长。

---

## 4. Proposed Solution

### 4.1 整体架构

```
┌──────────────────────────────────────────────────────────┐
│                   Luft (facade)                         │
│                                                           │
│  ┌────────────────────────────────────────────────┐    │
│  │                  Tool System                   │    │
│  │  ┌────────────┐  ┌────────────┐               │    │
│  │  │ Tool trait │  │ ToolRegistry │              │    │
│  │  └────────────┘  └────────────┘               │    │
│  └────────────────────────────────────────────────┘    │
│                                                           │
│  ┌────────────────────────────────────────────────┐    │
│  │               Shared State                     │    │
│  │  Arc<Luft>, RunRegistry, ReportStore           │    │
│  └────────────────────────────────────────────────┘    │
│                                                           │
│  ┌────────────────────────────────────────────────┐    │
│  │              Tools (2 items)                   │    │
│  │  ┌──────────────────┐  ┌─────────────────┐     │    │
│  │  │ExecuteWorkflowTool│  │   GetRunTool    │     │    │
│  │  └──────────────────┘  └─────────────────┘     │    │
│  └────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
         ↑                                           ↑
         │ call()                                     │ call()
         │                                           │
┌────────▼───────────────────────────────────────────▼──────┐
│                   Lua SDK Runtime                          │
│   tool("execute_workflow", { script = "..." })            │
│   tool("get_run", { run_id = "...", event_limit = 10 })  │
└────────────────────────────────────────────────────────────┘
```

### 4.2 Tool Trait 设计

```rust
// luft/src/tools/tool.rs

#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名，唯一标识。
    fn name(&self) -> &str;

    /// LLM 可见的 schema（名称、描述、JSON Schema）。
    fn spec(&self) -> ToolSpec;

    /// 执行工具，返回结构化结果或错误。
    async fn call(&self, args: Value) -> Result<Value, ToolError>;
}
```

**设计原则**：

- 瘦接口，不传 ctx 参数
- 依赖通过 struct 字段注入（`Arc<Luft>`, `RunRegistry`, 等）
- 返回 `Value`，调用方决定序列化格式

### 4.3 工具定义

#### ToolSpec

```rust
// luft/src/tools/tool.rs

pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,  // JSON Schema
}
```

#### ToolError

```rust
// luft/src/tools/tool.rs

pub enum ToolError {
    InvalidInput(String),
    NotFound(String),
    Execution(String),
}
```

### 4.4 共享状态

```rust
// luft/src/tools/context.rs

#[derive(Clone)]
pub struct LuftToolContext {
    luft: Arc<Luft>,
    runs: RunRegistry,           // Arc<Mutex<HashMap<String, RunInfo>>>
    search_dirs: Vec<PathBuf>,
}

// luft/src/tools/run_registry.rs (新模块)

pub type RunRegistry = Arc<Mutex<HashMap<String, RunInfo>>>;

pub struct RunInfo {
    pub run_id: String,
    pub run_dir_name: String,
    pub started_at: DateTime<Utc>,
}

pub fn new_run_registry() -> RunRegistry {
    Arc::new(Mutex::new(HashMap::new()))
}
```

### 4.5 工具 1: ExecuteWorkflowTool

**Struct**：

```rust
// luft/src/tools/execute_workflow.rs

pub struct ExecuteWorkflowTool {
    luft: Arc<Luft>,
    runs: RunRegistry,
}
```

**Input Schema**：

```json
{
  "type": "object",
  "properties": {
    "script": {
      "type": "string",
      "description": "Inline Lua workflow script"
    },
    "path": {
      "type": "string",
      "description": "Path to .lua workflow file"
    },
    "args": {
      "type": "object",
      "description": "Workflow arguments, accessible as `args` in Lua"
    }
  }
}
```

`script` 和 `path` 二选一，`script` 优先。

**Output Schema**：

```json
{
  "run_id": "01920f... (UUID v7)",
  "run_dir": "task_1781980050",
  "status": "running"
}
```

**执行流程**：

```
1. resolve_script_source(args)  →  script: String
2. validate_workflow(&script)   →  ValidationResult (syntax + structure)
3. luft.start_script(&script)   →  RunHandle
4. runs.lock().insert(run_id, RunInfo { run_dir_name })
5. 返回 { run_id, run_dir, status: "running" }
```

验证失败时返回 `Err(ToolError::InvalidInput(...))`。

### 4.6 工具 2: GetRunTool

**Struct**：

```rust
// luft/src/tools/get_run.rs

pub struct GetRunTool {
    luft: Arc<Luft>,
    runs: RunRegistry,
}
```

**Input Schema**：

```json
{
  "type": "object",
  "properties": {
    "run_id": {
      "type": "string",
      "description": "Run identifier (UUID v7)"
    },
    "since_event_id": {
      "type": "string",
      "description": "Only return events after this event ID (for incremental polling)"
    },
    "event_limit": {
      "type": "integer",
      "default": 0,
      "description": "Max events to return. 0 = no events."
    }
  },
  "required": ["run_id"]
}
```

**Output Schema**：

```json
{
  "status": {
    "run_id": "01920f...",
    "run_dir": "task_1781980050",
    "task": "Analyze codebase for security issues",
    "status": "running",
    "current_phase": 2,
    "completed_phases": 1,
    "total_started": 5,
    "completed_agents": 3,
    "running_agents": 2,
    "total_tokens": 12800,
    "created_at": "2025-07-16T10:30:00+00:00",
    "updated_at": "2025-07-16T10:35:00+00:00",
    "agents": [
      {
        "agent_id": "01J3a...",
        "name": "security-scanner",
        "role": "analyzer",
        "model": "claude-sonnet-4-20250514",
        "description": "Scan for injection vulnerabilities",
        "phase_id": 2,
        "agent_seq": 4,
        "started_at": "2025-07-16T10:32:00+00:00",
        "elapsed_ms": 180000,
        "tool_calls": 12,
        "tokens": {
          "prompt_tokens": 3400,
          "completion_tokens": 2100,
          "total_tokens": 5500
        }
      }
    ]
  },
  "events": []
}
```

**执行流程**：

```
1. resolve_run_dir(runs, run_id)  →  run_dir: String
2. luft.status(&run_dir)          →  Option<StatusOutput>
3. luft.events(&run_dir)          →  Vec<AgentEvent>
4. 若 since_event_id 存在，filter_events_since(events, since_event_id)
5. 若 event_limit > 0，tail(events, event_limit)
6. 构建 agents 数组（见下文）
7. 合并返回 { status, events }
```

#### `agents` 数组构建逻辑

```
running_ids = checkpoint.started_agent_ids - checkpoint.agent_results.keys()

if running_ids 为空:
    agents: []
else:
    for each running_id in running_ids:
        扫描 events，找该 agent 的最后一条 AgentStarted
        提取: agent_id, name, role, model, description, phase_id, agent_seq, started_at
        elapsed_ms = Utc::now() - started_at
        累计 tool_calls = count(AgentProgress::ToolCall for this agent)
        累计 tokens = sum(AgentProgress::Tokens for this agent)
```

| 字段 | 来源 |
|---|---|
| `agent_id`, `name`, `role`, `model`, `description`, `phase_id`, `agent_seq` | 最后一条 `AgentStarted` |
| `started_at` | `AgentStarted.ts` |
| `elapsed_ms` | `Utc::now() - AgentStarted.ts` |
| `tool_calls` | 累计 `AgentProgress::ToolCall` |
| `tokens` | 累计 `AgentProgress::Tokens` |

### 4.7 ToolRegistry

```rust
// luft/src/tools/registry.rs

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self;

    pub fn register(&mut self, tool: Box<dyn Tool>);

    /// 列出所有 tool spec，供 LLM function calling。
    pub fn specs(&self) -> Vec<ToolSpec>;

    /// 按 name 查找并调用。
    pub async fn call(&self, name: &str, args: Value) -> Result<Value, ToolError>;
}
```

不需要 `Arc<Mutex<>>`（Lua VM 单线程）。

### 4.8 System Prompt 构建

原 MCP resources（`workflow://schema`, `workflow://examples`）内嵌到 system prompt：

```rust
// luft/src/tools/system_prompt.rs

/// 构建 workflow 相关的系统 prompt 片段。
/// 包含 Lua DSL 语法参考 + 可用 workflow 列表。
pub fn build_workflow_prompt(search_dirs: &[PathBuf]) -> String {
    let mut s = String::new();

    // 1. Lua DSL 参考（原 workflow://schema）
    s.push_str(include_str!("../lua_dsl_reference.md"));

    // 2. 可用 workflow 列表（原 workflow://examples）
    let examples = list_examples(search_dirs);
    if !examples.is_empty() {
        s.push_str("\n\n## Available Workflows\n\n");
        for e in &examples {
            s.push_str(&format!("- **{}**: {}\n", e.name, e.description));
        }
    }

    s
}

/// 复用 luft-mcp/resources.rs 的 list_examples 逻辑。
fn list_examples(search_dirs: &[PathBuf]) -> Vec<ExampleEntry> {
    // 扫描 .lua 文件，提取 meta.reasoning 或首行注释
}
```

---

## 5. File Structure

```
luft/src/tools/
├── mod.rs                  # Tool trait, ToolSpec, ToolError, ToolRegistry
├── context.rs              # LuftToolContext
├── run_registry.rs         # RunRegistry, RunInfo
├── execute_workflow.rs     # ExecuteWorkflowTool
├── get_run.rs              # GetRunTool
└── system_prompt.rs        # build_workflow_prompt, list_examples
```

`luft/src/tools/mod.rs` 公开：

```rust
pub use tool::{Tool, ToolSpec, ToolError};
pub use registry::ToolRegistry;
pub use run_registry::{RunRegistry, RunInfo, new_run_registry};
pub use execute_workflow::ExecuteWorkflowTool;
pub use get_run::GetRunTool;
pub use system_prompt::build_workflow_prompt;
```

---

## 6. Deleted Code

| 删除项 | 原位置 | 理由 |
|---|---|---|
| `luft-mcp` crate 整个 | `crates/luft-mcp/` | stdio server、JSON-RPC、initialize/resources 协议全不需要 |
| `luft/src/mcp.rs` 整个 | `luft/src/mcp.rs` | data-plane MCP server 不需要（工具已被移除） |
| `JsonRpcMessage/Response/Error` | `luft-mcp/src/protocol.rs` | JSON-RPC 协议层 |
| `McpServer` + stdio loop | `luft-mcp/src/server.rs` | stdio 传输层 |
| `WorkflowUri` + resource read | `luft-mcp/src/resources.rs` | resource 协议层（改为 system prompt） |
| `McpRequest/McpResponse` | `luft/src/mcp.rs` | 重复的 JSON-RPC 类型 |
| `run_mcp_server()` | `luft/src/mcp.rs` | stdio 事件循环 |
| `luft mcp serve` 命令 | `luft-cli/src/commands/mcp_server.rs` | CLI 入口 |
| workspace 成员 `luft-mcp` | `Cargo.toml` | workspace 配置 |

---

## 7. Retained & Renamed

| 原名 | 新名 | 保留内容 |
|---|---|---|
| `McpStore` | ReportStore（如果需要）| data-plane 工具被移除，此结构可能不再需要 |
| `RunRegistry` | `RunRegistry`（不变） | run_id → run_dir_name 映射（新增模块） |
| `list_examples()` | 移入 `tools/system_prompt.rs` | 扫描 .lua 文件的逻辑 |

---

## 8. Implementation Plan

### Phase 1: Tool 基础设施

1. 创建 `luft/src/tools/` 模块结构
2. 实现 `Tool` trait + `ToolSpec` + `ToolError`
3. 实现 `ToolRegistry`（无锁，`Vec<Box<dyn Tool>>`）
4. 实现 `RunRegistry` + `RunInfo`

### Phase 2: 工具实现

1. 实现 `ExecuteWorkflowTool`
   - 集成 `luft_planner::validate_workflow`
   - 调用 `luft.start_script()`
   - 注册到 `RunRegistry`

2. 实现 `GetRunTool`
   - 调用 `luft.status()` 和 `luft.events()`
   - 实现 `agents` 数组构建逻辑
   - 支持 `since_event_id` 和 `event_limit`

### Phase 3: System Prompt 集成

1. 移植 `list_examples()` 到 `system_prompt.rs`
2. 实现 `build_workflow_prompt()`
3. 在 agent 初始化时注入到 system prompt

### Phase 4: Lua SDK 集成

1. 在 Lua runtime 中新增 `tool(name, args)` primitive
2. 注册 `ToolRegistry` 到 VM context
3. 生成工具列表的 system prompt 片段（可选，LLM function calling）

### Phase 5: 删除旧代码

1. 删除 `crates/luft-mcp/` 整个 crate
2. 删除 `luft/src/mcp.rs`
3. 删除 CLI `luft mcp serve` 命令
4. 更新 `Cargo.toml` workspace 成员
5. 更新文档

---

## 9. Testing Strategy

### 单元测试

- `ExecuteWorkflowTool`: 验证脚本解析、验证失败、run 注册
- `GetRunTool`: 验证状态查询、事件过滤、agents 数组构建、event_limit

### 集成测试

- Lua 脚本调用 `tool("execute_workflow", ...)`
- Lua 脚本轮询 `tool("get_run", ...)`

### 兼容性测试

- 加载旧 checkpoint（无 `ts` 字段）应不报错（`#[serde(default)]`）
- 旧 MCP 客户端不再连接后系统仍正常运行

---

## 10. Open Questions

### Q1: Data-plane 工具是否彻底删除？

原 `report_finding`、`report_artifacts`、`report_log`、`report_status`、`request_next_task` 不在最小集合内。

**决策**: 彻底删除。如有需要，后续可以独立工具形式添加。

### Q2: ReportStore 是否保留？

如果不再有 data-plane 工具，`ReportStore` 可能不再需要。

**决策**: 先保留，如果确认无其他用途再删除。

### Q3: ToolRegistry 是否需要锁？

Lua VM 单线程，理论上不需要锁。但如果有异步调用场景（未来），`Arc<Mutex<>>` 更安全。

**决策**: 当前使用 `Arc<Mutex<>>`（向 Loom 对齐），代价极小。

---

## Appendix A: Example Usage

### Lua 脚本调用示例

```lua
-- meta = { reasoning = "fire-and-forget workflow", phases = {} }
function main()
    local run = tool("execute_workflow", {
        script = [[
            meta = { reasoning = "nested", phases = {} }
            function main()
                local r = agent({ prompt = "Do something" })
                report(r.output)
            end
        ]]
    })
    print("Started:", run.run_id)

    -- 轮询状态
    local status = nil
    repeat
        local result = tool("get_run", {
            run_id = run.run_id,
            event_limit = 20
        })
        status = result.status
        print("Status:", status.status, "Phase:", status.current_phase)
        task.sleep(2)
    until status.status ~= "running"

    print("Done:", status.status)
end
```

---

## Appendix B: Related Documents

- Maestro 事件系统增强（commit `17ff921`）
- Loom Tool Core 文档（`loom/agent/tool/tool-core/`）
