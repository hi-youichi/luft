# Tool Core — 工具执行核心抽象

> **状态**: 设计阶段
> **目标**: 在 `luft-core` 中建立协议无关、强类型的工具执行核心。MCP server 和 builtin tool 都是它的上层封装；JSON 只存在于传输边界。

---

## 1. 分层架构

```
┌─────────────────────────────────────────────────────────────┐
│  传输层（thin adapters，唯一接触 JSON-RPC 的地方）            │
│                                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────────┐  │
│  │ MCP Server   │  │ MCP Client   │  │ ACP Bridge        │  │
│  │ (stdio/http) │  │ (future)     │  │ (session tools)   │  │
│  └──────┬───────┘  └──────┬───────┘  └────────┬──────────┘  │
│         │                 │                    │             │
│         │  serde_json::Value                   │             │
│         ▼                 ▼                    ▼             │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  Tool Registry + Middleware Pipeline                │    │
│  │  (luft-core/src/tool/)                              │    │
│  │                                                     │    │
│  │  JSON → typed Input → middleware → Tool::call       │    │
│  │  → typed Output → middleware → JSON                 │    │
│  └──────────────────────┬──────────────────────────────┘    │
│                         │                                   │
│                         ▼                                   │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  Typed Tool implementations                         │    │
│  │                                                     │    │
│  │  ┌─────────────┐  ┌─────────────┐  ┌────────────┐  │    │
│  │  │ Builtin     │  │ MCP Remote  │  │ Dynamic    │  │    │
│  │  │ (workflow,  │  │ (proxy to   │  │ (runtime   │  │    │
│  │  │  report,    │  │  external   │  │  registered│  │    │
│  │  │  structured)│  │  MCP server)│  │  per-task) │  │    │
│  │  └─────────────┘  └─────────────┘  └────────────┘  │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

**核心原则**：

- `luft-core/src/tool/` 是**唯一的工具执行引擎**——它不知道 MCP、不知道 ACP、不知道 JSON-RPC
- **工具作者只写 Rust 结构体**：每个工具定义自己的 `Input: DeserializeOwned` 和 `Output: Serialize`，`call()` 签名是强类型的
- **JSON 只在边界出现**：Registry 的类型擦除层负责 `serde_json::Value → Input` 和 `Output → serde_json::Value`，工具实现不接触 `serde_json::Value`
- MCP server 只是一个**传输适配器**：把 JSON-RPC 请求交给 Registry，把 Registry 返回的结果翻译回 JSON-RPC 响应
- 新增工具 = 定义 Input/Output 结构体 + 实现 `TypedTool` trait + 注册到 Registry

---

## 2. 核心类型（`luft-core/src/tool/`）

### 2.1 TypedTool trait（工具作者面对的接口）

```rust
/// 工具作者实现的 trait。全链路强类型，不接触 serde_json::Value。
///
/// 实现者只关心：
/// 1. 我的输入是什么结构（Input）
/// 2. 我的输出是什么结构（Output）
/// 3. 我怎么执行（call）
///
/// 不关心：JSON 反序列化、schema 校验、权限、超时、输出截断、协议序列化。
#[async_trait::async_trait]
pub trait TypedTool: Send + Sync + 'static {
    /// 输入参数类型。从传输层的 JSON 自动反序列化。
    type Input: DeserializeOwned + Send + 'static;
    /// 输出类型。由传输层自动序列化为 JSON。
    type Output: Serialize + Send + 'static;

    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn scope(&self) -> ToolScope;
    fn output_hint(&self) -> ToolOutputHint { ToolOutputHint::default() }

    async fn call(&self, input: Self::Input, ctx: &ToolContext) -> Result<Self::Output, ToolError>;
}
```

**工具作者写出来的代码**：

```rust
#[derive(Debug, Deserialize)]
struct WorkflowExecuteInput {
    script: Option<String>,
    path: Option<String>,
    args: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Serialize)]
struct WorkflowExecuteOutput {
    run_id: String,
    status: String,
}

struct WorkflowExecuteTool { luft: Arc<Luft> }

#[async_trait::async_trait]
impl TypedTool for WorkflowExecuteTool {
    type Input = WorkflowExecuteInput;
    type Output = WorkflowExecuteOutput;

    fn name(&self) -> &str { "workflow.execute" }
    fn description(&self) -> &str { "Execute a Luft workflow" }
    fn scope(&self) -> ToolScope { ToolScope::Global }

    async fn call(&self, input: Self::Input, ctx: &ToolContext) -> Result<Self::Output, ToolError> {
        let script = input.script
            .ok_or_else(|| ToolError::InvalidInput("missing 'script'".into()))?;
        let handle = self.luft.start_script(&script).await
            .map_err(|e| ToolError::Internal(e.into()))?;
        Ok(WorkflowExecuteOutput {
            run_id: handle.run_id().to_string(),
            status: "running".to_string(),
        })
    }
}
```

注意：`call()` 的签名里没有任何 `serde_json::Value`。`args` 字段用 `Value` 是因为 workflow 参数本身是动态的——这是业务语义，不是协议泄漏。

### 2.2 类型擦除（Registry 内部，工具作者不接触）

Registry 需要存储不同类型的工具，所以内部做类型擦除。JSON 转换只在这里发生：

```rust
/// Registry 内部使用的类型擦除 trait。工具作者不直接实现这个。
#[async_trait::async_trait]
pub(crate) trait ErasedTool: Send + Sync {
    fn name(&self) -> &str;
    fn descriptor(&self) -> ToolDescriptor;
    fn scope(&self) -> ToolScope;

    /// 接收 JSON，内部反序列化为 TypedTool::Input，执行，再把 Output 序列化为 JSON。
    async fn call_erased(&self, args: serde_json::Value, ctx: &ToolContext)
        -> Result<ErasedOutput, ToolError>;
}

/// 擦除后的输出。Registry 和传输层之间传递。
pub struct ErasedOutput {
    pub value: serde_json::Value,
    pub metadata: ToolResultMeta,
}

///  blanket impl：任何 TypedTool 自动获得 ErasedTool 实现。
#[async_trait::async_trait]
impl<T: TypedTool> ErasedTool for T {
    fn name(&self) -> &str { TypedTool::name(self) }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: generate_schema::<T::Input>(),
            output_hint: self.output_hint(),
        }
    }

    fn scope(&self) -> ToolScope { TypedTool::scope(self) }

    async fn call_erased(&self, args: serde_json::Value, ctx: &ToolContext)
        -> Result<ErasedOutput, ToolError>
    {
        let input: T::Input = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let output = self.call(input, ctx).await?;
        let value = serde_json::to_value(output)
            .map_err(|e| ToolError::Internal(e.into()))?;
        Ok(ErasedOutput { value, metadata: Default::default() })
    }
}
```

**JSON 只出现在 `call_erased` 的两行 `from_value` / `to_value` 里。** 工具作者的 `call()` 完全不接触 JSON。

### 2.3 描述符

```rust
/// 工具元数据。input_schema 从 Input 类型自动派生，不需要手写 JSON Schema。
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    /// 从 TypedTool::Input 自动生成的 JSON Schema（draft-07）。
    /// 用 schemars crate 的 schema_for! 宏生成。
    pub input_schema: serde_json::Value,
    pub output_hint: ToolOutputHint,
}

/// 输出大小特征，驱动 OutputMiddleware 的截断/落盘决策。
#[derive(Debug, Clone, Default)]
pub struct ToolOutputHint {
    pub safe_inline_chars: Option<usize>,
    pub prefer_head_tail: bool,
}
```

`input_schema` 的生成：

```rust
fn generate_schema<T: DeserializeOwned + schemars::JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or_default()
}
```

这要求 `TypedTool::Input` 额外实现 `schemars::JsonSchema`。trait bound 更新为：

```rust
type Input: DeserializeOwned + schemars::JsonSchema + Send + 'static;
```

**好处**：schema 和结构体是同一个来源，不会出现"改了结构体忘了改 schema"的问题。当前三套实现里手写的 `serde_json::json!({...})` schema 全部消除。

### 2.4 执行上下文

```rust
/// 由 Registry 注入的可信上下文。工具实现不自己解析身份。
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub run_id: Option<RunId>,
    pub agent_id: Option<AgentId>,
    pub task_id: Option<TaskId>,
    pub scope: ToolScope,
    pub policy: ToolPolicy,
    pub deadline: Option<std::time::Instant>,
    pub cancel: tokio_util::sync::CancellationToken,
}

/// 工具可见范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolScope {
    Global,  // 外部 MCP client（Claude Code 等）
    Run,     // 当前 run 的 agent
    Task,    // 特定 task 的 agent
}
```

### 2.5 错误

```rust
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// Input 反序列化失败（JSON → typed struct）
    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("tool not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    Denied(String),

    #[error("timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("cancelled")]
    Cancelled,

    #[error("scope mismatch: {tool} requires {required:?}, connection is {actual:?}")]
    ScopeMismatch { tool: String, required: ToolScope, actual: ToolScope },

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}
```

注意：没有 `SchemaInvalid` 变体了。schema 校验由 `SchemaMiddleware` 在 `call_erased` 之前做（用 `input_schema` 校验原始 JSON），反序列化失败走 `InvalidInput`。

---

## 3. Registry（执行引擎）

```rust
pub struct ToolRegistry {
    tools: RwLock<Vec<Arc<dyn ErasedTool>>>,
    middleware: Vec<Arc<dyn ToolMiddleware>>,
}

impl ToolRegistry {
    /// 注册工具。接受任何 TypedTool，内部自动擦除。
    pub fn register<T: TypedTool>(&self, tool: T) -> Result<(), ToolError> {
        let name = tool.name().to_string();
        let mut tools = self.tools.write().unwrap();
        if tools.iter().any(|t| t.name() == name) {
            return Err(ToolError::Internal(anyhow!("duplicate tool name: {name}")));
        }
        tools.push(Arc::new(tool));
        Ok(())
    }

    /// 列出在指定 scope 下可见的工具。
    pub fn list(&self, scope: ToolScope) -> Vec<ToolDescriptor>;

    /// 执行一次调用。
    ///
    /// 这是 Registry 唯一接受 JSON 的公开方法——因为调用方（传输适配器）
    /// 拿到的就是 JSON。内部流程：
    ///   JSON args → SchemaMiddleware 校验 → call_erased 反序列化为 typed Input
    ///   → tool.call(typed Input) → typed Output → 序列化为 JSON → after middleware
    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ErasedOutput, ToolError>;
}
```

### Scope 可见性

连接 scope 越深，能看到的工具越多：

```
Global 连接 → 只看到 Global 工具
Run 连接    → 看到 Global + Run 工具
Task 连接   → 看到 Global + Run + Task 工具
```

---

## 4. Middleware 管线

```rust
#[async_trait::async_trait]
pub trait ToolMiddleware: Send + Sync {
    /// 调用前拦截。args 是原始 JSON（尚未反序列化为 typed Input）。
    async fn before(&self, name: &str, args: &mut serde_json::Value, ctx: &ToolContext)
        -> Result<(), ToolError>;

    /// 调用后拦截。output 是已序列化的 JSON（typed Output 已转为 Value）。
    async fn after(&self, name: &str, output: &mut ErasedOutput, ctx: &ToolContext);
}
```

Middleware 操作的是 JSON 边界——因为它们的职责（schema 校验、输出截断）本身就是在 JSON 层面做的。工具作者不接触 Middleware。

执行顺序：

```
execute(name, args: Value, ctx)
  │
  ├─ find(name) in scope → Arc<dyn ErasedTool>
  │
  ├─ before chain (操作 JSON):
  │    ScopeMiddleware   → 可见性检查
  │    SchemaMiddleware  → 用 descriptor.input_schema 校验 args
  │    PolicyMiddleware  → ToolPolicy allow/deny
  │    TimeoutMiddleware → 设置 tokio::time::timeout
  │
  ├─ tool.call_erased(args, ctx)
  │    内部: Value → T::Input → tool.call(input, ctx) → T::Output → Value
  │
  ├─ after chain (操作 JSON, reverse):
  │    OutputMiddleware  → 截断 / 落盘 / FileRef
  │    AuditMiddleware   → 记录日志
  │
  └─ return ErasedOutput { value: Value, metadata }
```

| Middleware | 替代的现有代码 |
|------------|---------------|
| `SchemaMiddleware` | `mcp_server.rs:191` 的 `validate_against_schema` + 三套手写 JSON Schema |
| `PolicyMiddleware` | `permission.rs:37` 的 `decide()` |
| `TimeoutMiddleware` | 当前缺失 |
| `OutputMiddleware` | 借鉴 Loom `tool_output_normalizer.rs` |
| `AuditMiddleware` | 当前缺失 |

---

## 5. 上层封装

### 5.1 Builtin Tools

每个 builtin tool 定义自己的 Input/Output 结构体，实现 `TypedTool`：

| 工具 | Input 结构体 | Output 结构体 | Scope |
|------|-------------|--------------|-------|
| `workflow.execute` | `WorkflowExecuteInput { script, path, args }` | `WorkflowExecuteOutput { run_id, status }` | Global |
| `workflow.list` | `WorkflowListInput {}` | `WorkflowListOutput { workflows: Vec<WorkflowInfo> }` | Global |
| `workflow.status` | `RunStatusInput { run_id }` | `RunStatusOutput { status, phases, ... }` | Global |
| `workflow.events` | `RunEventsInput { run_id, since_event_id }` | `RunEventsOutput { events: Vec<RunEvent> }` | Global |
| `structured_output` | 动态（schema 由 task 决定） | `StructuredOutputAck { accepted: bool }` | Task |
| `report.artifacts` | `ReportArtifactsInput { artifacts: Vec<Artifact> }` | `ReportAck { message: String }` | Run |
| `report.log` | `ReportLogInput { level, msg }` | `ReportAck { message: String }` | Run |
| `report.status` | `ReportStatusInput { status, progress, message }` | `ReportAck { message: String }` | Run |

**`structured_output` 的特殊性**：它的 Input schema 是动态的（由 workflow 的 output_schema 决定），不能用编译时 `schemars::schema_for!` 派生。处理方式：

```rust
struct StructuredOutputTool {
    schema: serde_json::Value,  // 运行时传入的 JSON Schema
}

#[async_trait::async_trait]
impl TypedTool for StructuredOutputTool {
    // Input 是 Value——这是业务语义（动态 schema），不是协议泄漏
    type Input = serde_json::Value;
    type Output = StructuredOutputAck;

    fn name(&self) -> &str { "structured_output" }
    fn description(&self) -> &str { "Submit your final result" }
    fn scope(&self) -> ToolScope { ToolScope::Task }

    async fn call(&self, input: Self::Input, ctx: &ToolContext) -> Result<Self::Output, ToolError> {
        validate_against_schema(&input, &self.schema)
            .map_err(|e| ToolError::InvalidInput(e))?;
        // 存储结果到 task 关联的 store
        Ok(StructuredOutputAck { accepted: true })
    }
}
```

这是唯一一个 `type Input = serde_json::Value` 的工具——因为它的 schema 本身就是运行时动态的。其他所有工具的 Input 都是编译时确定的结构体。

### 5.2 MCP Server Adapter

MCP server 变成纯传输层——只做 JSON-RPC ↔ Registry 的翻译：

```rust
pub struct McpServer {
    registry: Arc<ToolRegistry>,
    scope: ToolScope,
}

impl McpServer {
    async fn dispatch_method(&self, method: &str, params: &Value) -> Result<Value, (i32, String)> {
        match method {
            "tools/list" => {
                let descriptors = self.registry.list(self.scope);
                Ok(serialize_tools_list(&descriptors))
            }
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str())
                    .ok_or((INVALID_PARAMS, "missing 'name'"))?;
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let ctx = self.build_context();
                match self.registry.execute(name, args, &ctx).await {
                    Ok(output) => Ok(serialize_output(&output)),
                    Err(e) => Ok(serialize_error(&e)),
                }
            }
            // initialize / ping / resources/* 不变
            _ => Err((METHOD_NOT_FOUND, format!("method not found: {method}"))),
        }
    }
}
```

**三套 MCP 实现统一为同一个 `McpServer`，只是 scope 和注册的工具不同**：

| 入口 | scope | 注册的工具 |
|------|-------|-----------|
| `luft mcp serve` | Global | workflow.* 4 个 |
| `luft mcp-structured-output` | Task | structured_output |
| agent session 内 | Run | report.* 4 个 |

### 5.3 MCP Client Adapter（future）

未来调用外部 MCP server 的工具，对 Registry 来说和 builtin tool 没区别：

```rust
/// 代理到外部 MCP server 的工具。
/// Input/Output 都是 Value——因为外部工具的 schema 是运行时发现的。
struct McpRemoteTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    session: Arc<McpSession>,
}

#[async_trait::async_trait]
impl TypedTool for McpRemoteTool {
    type Input = serde_json::Value;   // 外部工具 schema 动态
    type Output = serde_json::Value;  // 外部工具返回动态

    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { &self.description }
    fn scope(&self) -> ToolScope { ToolScope::Global }

    async fn call(&self, input: Self::Input, _ctx: &ToolContext) -> Result<Self::Output, ToolError> {
        let response = self.session.call_tool(&self.name, &input).await
            .map_err(|e| ToolError::Internal(e.into()))?;
        Ok(response)
    }
}
```

---

## 6. 协议序列化（`luft-mcp/src/protocol.rs`）

唯一做 MCP 格式翻译的地方：

```rust
/// ToolDescriptor → MCP tools/list 项
pub fn serialize_tools_list(descriptors: &[ToolDescriptor]) -> Value {
    json!({
        "tools": descriptors.iter().map(|d| json!({
            "name": d.name,
            "description": d.description,
            "inputSchema": d.input_schema,
        })).collect::<Vec<_>>()
    })
}

/// ErasedOutput → MCP tools/call 响应
pub fn serialize_output(output: &ErasedOutput) -> Value {
    json!({
        "content": [{ "type": "text", "text": output.value.to_string() }],
        "isError": false
    })
}

/// ToolError → MCP tools/call 错误响应
pub fn serialize_error(err: &ToolError) -> Value {
    json!({
        "content": [{ "type": "text", "text": err.to_string() }],
        "isError": true
    })
}
```

---

## 7. 与 ACP 层的关系

| ACP 代码 | 当前行为 | 整合后 |
|----------|---------|--------|
| `acp_adapter.rs:611` session_new | 拼装 McpServerStdio 传给 agent | 不变——agent 仍通过 stdio 连 MCP，但 MCP server 内部走 Registry |
| `permission.rs:37` decide | 硬编码 `structured_output` 白名单 | 迁移到 PolicyMiddleware |
| `update_mapper.rs:87` | 按 title 字符串匹配捕获 rawInput | 不变——属于 ACP 协议适配层 |

---

## 8. 依赖变更

`luft-core/Cargo.toml` 新增：

```toml
schemars = "0.8"  # 从 Input 结构体自动派生 JSON Schema
```

`luft-mcp/Cargo.toml` 不变（已有 serde_json）。

---

## 9. 迁移路径

| 步骤 | 内容 | 影响 | 验证 |
|------|------|------|------|
| **M1** | `luft-core/src/tool/` 定义 `TypedTool`、`ErasedTool`、`ToolRegistry`、`ToolMiddleware`、核心类型 | 纯新增 | `cargo check -p luft-core` |
| **M2** | 实现 Registry + 6 个 Middleware + pipeline + `schemars` schema 派生 | 纯新增 | 单元测试 |
| **M3** | `luft-mcp` 的 4 个 handler → `TypedTool` impl（定义 Input/Output 结构体），server 委托 Registry | `luft-mcp` 内部 | 现有测试全通过 |
| **M4** | `mcp-structured-output` → `StructuredOutputTool` + Registry | `luft-cli` 内部 | E2E 测试通过 |
| **M5** | `luft/src/mcp.rs` 5 个工具 → `TypedTool` impl，agent_id 从 ctx 注入 | `luft` + scheduler | 现有测试 + 新增注入测试 |

每步可独立合并。M1-M2 不碰任何现有代码。

---

## 10. 开放问题

| # | 问题 | 倾向 |
|---|------|------|
| 1 | Registry 生命周期：每进程一个 vs 全局单例 | 每进程一个；agent session 内动态注册 Run/Task 工具 |
| 2 | ToolPolicy 放 `luft-core` 还是 `luft-adapters` | `luft-core`（与 ToolContext 同层） |
| 3 | 工具名 namespace 化时机 | M3 保持原名兼容外部 client，M5 统一改 |
| 4 | OutputMiddleware 落盘位置 | `{base_dir}/runs/{run_id}/tool-outputs/` |
| 5 | MCP Client 何时实现 | 有外部工具调用需求时再加，不影响核心设计 |
| 6 | `schemars` 版本与 JSON Schema draft 兼容性 | 用 `schemars 0.8`（draft-07），与 MCP 协议兼容 |
