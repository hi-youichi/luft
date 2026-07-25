# Tool 参考

> Luft 工具系统的开发者参考：实现、注册、调用、扩展。

---

## 1. 概述

Luft 的所有可调用能力（workflow 执行、结构化输出提交、agent reporting）统一为 `Tool` 抽象：

- **核心 trait**：`TypedTool`（`luft-core/src/tool/`），强类型 `Input`/`Output`
- **执行引擎**：`ToolRegistry`，唯一注册/查找/执行入口
- **传输层**：MCP server / future MCP client / ACP bridge
- **横切关注点**：Middleware（scope、schema、policy、timeout、output、audit）

工具作者只写 Rust 结构体，不接触 JSON / JSON-RPC / MCP。JSON 转换在 Registry 内部自动完成。

---

## 2. 实现一个新工具

### 2.1 三步流程

```
1. 定义 Input/Output 结构体（强类型，serde 派生）
2. impl TypedTool for YourTool
3. registry.register(your_tool)?
```

### 2.2 完整示例

```rust
use luft_core::tool::{TypedTool, ToolScope, ToolContext, ToolError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── 1. 定义输入输出 ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct EchoInput {
    /// 要回显的消息
    message: String,
    /// 重复次数
    repeat: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct EchoOutput {
    echoed: String,
    repeated: u32,
}

// ── 2. 实现 TypedTool ────────────────────────────────────────

struct EchoTool {
    max_repeat: u32,
}

#[async_trait::async_trait]
impl TypedTool for EchoTool {
    type Input = EchoInput;
    type Output = EchoOutput;

    fn name(&self) -> &str { "echo" }
    fn description(&self) -> &str { "Echo back the message N times" }
    fn scope(&self) -> ToolScope { ToolScope::Global }

    async fn call(&self, input: Self::Input, _ctx: &ToolContext) -> Result<Self::Output, ToolError> {
        let repeat = input.repeat.unwrap_or(1).min(self.max_repeat);
        Ok(EchoOutput {
            echoed: input.message.repeat(repeat as usize),
            repeated: repeat,
        })
    }
}

// ── 3. 注册 ──────────────────────────────────────────────────

registry.register(EchoTool { max_repeat: 10 })?;
```

### 2.3 约束

| 约束 | 原因 |
|------|------|
| `Input: DeserializeOwned + JsonSchema` | 反序列化 + 派生 JSON Schema |
| `Output: Serialize` | 序列化为 JSON 传给传输层 |
| `Send + Sync + 'static` | 存入 `Arc<dyn ErasedTool>` |
| 结构体字段命名 | 用 `snake_case`，匹配 MCP 惯例 |
| 必填字段 | 直接用非 `Option` 类型，schema 自动标记 required |

---

## 3. 内置工具清单

### 3.1 Global Scope — 工作流控制面

由 `luft mcp serve` 暴露给外部 MCP client（Claude Code 等）。

#### `workflow.execute`

执行一个 Luft workflow。

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct WorkflowExecuteInput {
    /// 内联 Lua 脚本
    script: Option<String>,
    /// .lua 文件路径（相对 CWD）
    path: Option<String>,
    /// 传给 workflow 的参数，Lua 中可通过 `args` 访问
    args: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct WorkflowExecuteOutput {
    run_id: String,
    status: String,  // 总是 "running"
}
```

**前置校验**：执行前调用 `validate_workflow`（语法 → 结构 → schema）。失败时不启动，直接返回错误。

**返回模式**：fire-and-forget。立即返回 `run_id`，不阻塞。用 `workflow.status` 轮询进度。

#### `workflow.list`

列出可用的 workflow 文件。

```rust
#[derive(Debug, Default, Deserialize)]
struct WorkflowListInput {}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct WorkflowListOutput {
    workflows: Vec<WorkflowInfo>,
}

struct WorkflowInfo {
    name: String,
    path: String,
    description: Option<String>,
}
```

**来源**：扫描 `workflows/` 和 `examples/` 目录。`description` 从每个 `.lua` 文件的 `meta.reasoning` 字段提取。

#### `workflow.status`

查询 run 的当前状态。

```rust
#[derive(Debug, Deserialize)]
struct RunStatusInput {
    run_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct RunStatusOutput {
    run_id: String,
    task: String,
    status: CheckpointStatus,
    current_phase: u32,
    total_phases: u32,
    total_tokens: u64,
    elapsed_secs: Option<f64>,
    completed_agents: u32,
    running_agents: u32,
    phases: Vec<PhaseInfo>,
    report: Option<serde_json::Value>,
    error: Option<String>,
}

struct PhaseInfo {
    phase_id: u32,
    label: String,
    detail: Option<String>,
    status: PhaseStatus,
    planned: Option<u32>,
    ok: u32,
    failed: u32,
    agents: Vec<AgentInfo>,
}
```

**边界情况**：
- `run_id` 不存在 → `ToolError::NotFound`
- run 正在运行 → `report` 和 `error` 为 `None`
- run 被取消 → `status: Cancelled`

#### `workflow.events`

查询 run 的事件流。

```rust
#[derive(Debug, Deserialize)]
struct RunEventsInput {
    run_id: String,
    /// 增量轮询：只返回此 ID 之后的事件
    since_event_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct RunEventsOutput {
    events: Vec<RunEvent>,
}

struct RunEvent {
    event_id: String,
    #[serde(rename = "type")]
    event_type: String,
    agent: Option<String>,
    ts: u64,
    payload: Option<serde_json::Value>,
}
```

### 3.2 Task Scope — 结构化输出

由 `luft mcp-structured-output` 暴露给当前 task 的 agent。

#### `structured_output`

提交 task 的最终结构化结果。

```rust
// 这是唯一一个 type Input = serde_json::Value 的工具
// 因为 schema 由 workflow 运行时决定，不能编译期派生

struct StructuredOutputTool {
    /// workflow 提供的 output_schema
    schema: serde_json::Value,
}

impl TypedTool for StructuredOutputTool {
    type Input = serde_json::Value;   // 业务语义：动态 schema
    type Output = StructuredOutputAck;

    async fn call(&self, input: Self::Input, ctx: &ToolContext) -> Result<Self::Output, ToolError> {
        validate_against_schema(&input, &self.schema)
            .map_err(|e| ToolError::InvalidInput(e))?;
        // 存储到 ctx.task_id 关联的结果 store
        Ok(StructuredOutputAck { accepted: true })
    }
}

#[derive(Debug, Serialize)]
struct StructuredOutputAck {
    accepted: bool,
}
```

**为什么 Input 是 `Value`**：这个工具的 schema 不是工具作者决定的，而是由 workflow 的 `output_schema` 决定。属于业务语义，不是协议泄漏。

### 3.3 Run Scope — Agent 上报

由 agent session 内部的 MCP 连接暴露。

#### `report.artifacts`

上报生成的产物。

```rust
#[derive(Debug, Deserialize)]
struct ReportArtifactsInput {
    artifacts: Vec<ArtifactInput>,
}

#[derive(Debug, Deserialize)]
struct ArtifactInput {
    key: String,
    path: Option<String>,
    inline: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ReportAck { message: String }
```

#### `report.log`

上报日志条目。

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ReportLogInput {
    level: String,        // "trace" | "debug" | "info" | "warn" | "error"
    msg: String,
}

#[derive(Debug, Serialize)]
struct ReportAck { message: String }
```

#### `report.status`

上报 agent 进度或完成状态。

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ReportStatusInput {
    status: String,        // "progress" | "completed" | "failed"
    progress: Option<f32>, // 0.0 ~ 1.0
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReportAck { message: String }
```

---

## 4. Registry API

```rust
// 创建
let registry = ToolRegistry::new();

// 注册（接受任何 TypedTool，内部自动擦除）
registry.register(workflow_execute_tool)?;

// 列出在某个 scope 下可见的工具
let descriptors = registry.list(ToolScope::Global);

// 执行（传输层调用方拿到的就是 JSON args）
let output = registry.execute("workflow.execute", args, &ctx).await?;
```

**Register 约束**：
- 重名返回 `ToolError::Internal("duplicate tool name")`
- 不静默覆盖——注册失败要立刻知道

**Execute 内部流程**：
```
execute(name, args: Value, ctx)
  ├─ find(name) in scope
  ├─ before middleware chain（操作 JSON）
  ├─ tool.call_erased(args, ctx)
  │    内部: Value → T::Input → tool.call(input, ctx) → T::Output → Value
  ├─ after middleware chain（操作 JSON）
  └─ return ErasedOutput { value, metadata }
```

---

## 5. Middleware

```rust
#[async_trait::async_trait]
pub trait ToolMiddleware: Send + Sync {
    async fn before(&self, name: &str, args: &mut serde_json::Value, ctx: &ToolContext)
        -> Result<(), ToolError>;
    async fn after(&self, name: &str, output: &mut ErasedOutput, ctx: &ToolContext);
}
```

### 5.1 内置 Middleware

| Middleware | 阶段 | 职责 |
|------------|------|------|
| `ScopeMiddleware` | before | 检查 `name` 在 `ctx.scope` 下可见 |
| `SchemaMiddleware` | before | 用 `descriptor.input_schema` 校验 `args` |
| `PolicyMiddleware` | before | `ToolPolicy.decide()` allow/deny |
| `TimeoutMiddleware` | before | 设置 `tokio::time::timeout(ctx.deadline)` |
| `OutputMiddleware` | after | 大输出截断/落盘/FileRef |
| `AuditMiddleware` | after | 记录调用日志 |

### 5.2 自定义 Middleware

```rust
struct RateLimitMiddleware {
    max_calls_per_min: u32,
    counter: AtomicU32,
}

#[async_trait::async_trait]
impl ToolMiddleware for RateLimitMiddleware {
    async fn before(&self, name: &str, _args: &mut Value, _ctx: &ToolContext) -> Result<(), ToolError> {
        let count = self.counter.fetch_add(1, Ordering::Relaxed);
        if count >= self.max_calls_per_min {
            return Err(ToolError::Denied(format!("rate limit exceeded for {name}")));
        }
        Ok(())
    }

    async fn after(&self, _name: &str, _output: &mut ErasedOutput, _ctx: &ToolContext) {}
}

registry.add_middleware(Arc::new(RateLimitMiddleware { ... }));
```

---

## 6. Scope 规则

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolScope {
    Global,  // 外部 MCP client
    Run,     // 当前 run 的 agent
    Task,    // 特定 task 的 agent
}
```

可见性：

| Tool Scope | Global 连接可见 | Run 连接可见 | Task 连接可见 |
|-----------|---------------|------------|-------------|
| Global | ✅ | ✅ | ✅ |
| Run | ❌ | ✅ | ✅ |
| Task | ❌ | ❌ | ✅ |

**经验法则**：连接的 scope 越深，能看到的工具越多。`Task` 连接能看到全部。

**Scope 验证**：当前 run 的 agent 尝试调用 `workflow.execute` → `ToolError::ScopeMismatch`。

---

## 7. 输出截断（OutputMiddleware）

当 Output 超过 `ToolOutputHint.safe_inline_chars` 时：

1. 落盘到 `{base_dir}/runs/{run_id}/tool-outputs/{tool_name}-{timestamp}.json`
2. `ErasedOutput.value` 变成 `FileRef { path, excerpt }`
3. 传输层把 FileRef 序列化为 `"Output persisted to {path}."`

工具作者通过 `output_hint()` 声明这个工具的输出可能有多大：

```rust
fn output_hint(&self) -> ToolOutputHint {
    ToolOutputHint {
        safe_inline_chars: Some(4000),  // 超过 4000 字符触发落盘
        prefer_head_tail: false,
    }
}
```

小输出工具（`report_*` 的 ack）用默认值或显式声明 `always_inline`：

```rust
fn output_hint(&self) -> ToolOutputHint {
    ToolOutputHint::always_inline()
}
```

---

## 8. 错误处理

```rust
pub enum ToolError {
    InvalidInput(String),   // JSON → typed struct 反序列化失败
    NotFound(String),       // 工具名找不到
    Denied(String),         // PolicyMiddleware 拒绝
    Timeout(Duration),      // 超时
    Cancelled,              // ctx.cancel 被触发
    ScopeMismatch { ... },  // scope 不匹配
    Internal(anyhow::Error) // 工具实现自己抛的
}
```

**工具作者的惯用法**：

```rust
async fn call(&self, input: Self::Input, ctx: &ToolContext) -> Result<Self::Output, ToolError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(ToolError::InvalidInput("name cannot be empty".into()));
    }
    if ctx.cancel.is_cancelled() {
        return Err(ToolError::Cancelled);
    }
    // 业务错误用 Internal（带上下文）
    let result = do_work(name).await
        .map_err(|e| ToolError::Internal(e.context("work failed for {name}")))?;
    Ok(result)
}
```

---

## 9. 测试

### 9.1 单元测试一个工具

```rust
#[tokio::test]
async fn test_echo_tool() {
    let tool = EchoTool { max_repeat: 10 };
    let ctx = ToolContext::for_test();

    // 直接调用（不走 Registry）
    let output = tool.call(
        EchoInput { message: "hi".into(), repeat: Some(3) },
        &ctx,
    ).await.unwrap();
    assert_eq!(output.echoed, "hihihi");
    assert_eq!(output.repeated, 3);
}
```

### 9.2 集成测试（经过 Registry）

```rust
#[tokio::test]
async fn test_registry_execute() {
    let registry = ToolRegistry::new();
    registry.register(EchoTool { max_repeat: 10 }).unwrap();

    let args = serde_json::json!({ "message": "hi", "repeat": 3 });
    let ctx = ToolContext::for_test().with_scope(ToolScope::Global);

    let output = registry.execute("echo", args, &ctx).await.unwrap();
    let parsed: EchoOutput = serde_json::from_value(output.value).unwrap();
    assert_eq!(parsed.echoed, "hihihi");
}
```

### 9.3 E2E 测试（经过 MCP）

见 `crates/luft-cli/tests/structured_output_flow.rs` 模式——用 stdio pipe 模拟完整 MCP 会话。

---

## 10. 添加新工具的 Checklist

- [ ] 定义 `Input` 结构体（带 `#[derive(Deserialize)]`，字段命名用 snake_case）
- [ ] 定义 `Output` 结构体（带 `#[derive(Serialize)]`）
- [ ] 添加 `JsonSchema` derive 到 `Input`（schemars 自动派生）
- [ ] 创建 `struct YourTool { ... }`
- [ ] `impl TypedTool for YourTool`：填 `name` / `description` / `scope` / `output_hint` / `call`
- [ ] 在适当的 `build_context()` 中调用 `registry.register(tool)?`
- [ ] 单测：直接调 `tool.call()` + 走 Registry
- [ ] 文档：更新本文件第 3 节的工具清单