# Structured Output MCP Schema 校验失败调查

> **状态**：已定位，待修复
> **调查对象**：`luft-workflow_1785732656` / `compare:health.get`
> **影响范围**：所有通过 ACP 注入 `luft-structured-output` 并使用 `schema` 的 agent
> **相关代码**：`crates/luft-cli/src/commands/mcp_server.rs`、`crates/luft-adapters/src/update_mapper.rs`、`crates/luft-adapters/src/result_collector.rs`、`crates/luft-core/src/scheduler/mod.rs`
> **交叉参考**：[agent-session-schema-retry.md](../design/agent-session-schema-retry.md)

---

## 1. 结论

本次失败不是模型没有生成业务结果，也不是 ACP session resume 失效，而是 MCP tool 的参数包装在 adapter 中被当成了 agent 的最终业务结果。

当前 `workflow_validate_schema` 的 MCP 参数契约是：

```json
{
  "input": { "<业务结果字段>": "..." },
  "schema": { "<JSON Schema>": "..." }
}
```

MCP server 正确地执行：

```text
validate(input, schema)
```

但 adapter 捕获完整的 `rawInput` 后，直接把下面这个 wrapper 交给 scheduler：

```json
{
  "input": {
    "endpoint": "GET /global/health",
    "opencode_files": [],
    "loom_files": [],
    "evidence": [],
    "differences": [],
    "changes": [],
    "acceptance": [],
    "out_of_scope": [],
    "summary": "..."
  },
  "schema": {
    "type": "object",
    "required": ["endpoint", "opencode_files"]
  }
}
```

scheduler 实际校验的是：

```text
validate({ input, schema }, PLAN_SCHEMA)
```

而不是：

```text
validate(input, PLAN_SCHEMA)
```

因此顶层缺少 `endpoint`、`opencode_files`、`loom_files` 等 required properties。

## 2. 源码事实

### 2.1 MCP tool 的真实参数定义

`luft-structured-output` 使用 RMCP 注册单个 tool。参数 struct 定义为：

```rust
struct WorkflowValidateSchemaInput {
    /// The data to validate.
    input: Value,
    /// JSON Schema (Draft 7) to validate against.
    schema: Value,
}
```

位置：`crates/luft-cli/src/commands/mcp_server.rs:86-107`。

tool handler 直接使用两个字段：

```rust
fn workflow_validate_schema(
    Parameters(params): Parameters<WorkflowValidateSchemaInput>,
) -> Result<String, String> {
    validate_against_schema(&params.input, &params.schema)
        .map(|_| "Result accepted.".to_string())
        .map_err(|e| format!("Schema validation failed: {e}"))
}
```

所以 MCP server 的行为本身是正确的。它并不知道 workflow 的 `PLAN_SCHEMA` 应该如何映射到 scheduler 的 `AgentResult.output`。

### 2.2 ACP adapter 如何注入 MCP server

当 agent 提供 `output_schema` 时，ACP adapter 在 `session/new` 和 `session/resume` 中注入：

```text
server name: luft-structured-output
command:     <luft binary>
args:        ["mcp-structured-output"]
```

位置：`crates/luft-adapters/src/acp_adapter.rs:908-920`。

### 2.3 rawInput 如何被捕获

`update_mapper` 通过 tool 标题识别 `workflow_validate_schema`，然后完整保存 `rawInput`：

```rust
if title.contains("workflow_validate_schema") {
    if let Some(raw_input) = ... {
        *acc.workflow_validate_schema.lock().unwrap() = Some(raw_input);
    }
}
```

位置：`crates/luft-adapters/src/update_mapper.rs:87-98`。

这里保存整个 `{ input, schema }` 是合理的采集行为；问题在于下一层没有解包。

### 2.4 result collector 没有提取 input

当前 `result_collector` 直接将捕获值作为输出：

```rust
let output = if let Some(json) = workflow_validate_schema {
    json
} else {
    // fallback paths
}
```

位置：`crates/luft-adapters/src/result_collector.rs:19-44`。

这里应该将 `workflow_validate_schema.input` 作为 `AgentResult.output`，而不是将整个 MCP 参数 wrapper 作为输出。

### 2.5 scheduler 校验的边界

scheduler 在拿到 `AgentResult` 后，使用 `task.output_schema` 校验 `result.output`：

```rust
validate_output(&result.output, schema)
```

位置：`crates/luft-core/src/scheduler/mod.rs` 的 schema validation 分支。

因此 scheduler 期望接收的是纯业务对象。它不应该认识 MCP 的 `input`、`schema`、`server` 或 `tool` 元数据。

## 3. 运行证据

运行：`luft-workflow_1785732656`。

### 3.1 agent 的真实行为

事件流显示 agent 完成了跨项目源码调查，并调用了 `workflow_validate_schema`。第一次提交后，scheduler 报告：

```text
output does not match schema:
instance : "endpoint" is a required property
instance : "opencode_files" is a required property
instance : "loom_files" is a required property
instance : "evidence" is a required property
instance : "differences" is a required property
```

失败反馈中的 `output` 是 MCP wrapper，而 wrapper 内的 `input` 已经包含完整的业务字段。

### 3.2 session resume 实际生效

事件流包含：

```text
schema_retry attempt=1
session/resume sessionId=019fc5f6-28d8-7252-94ee-e3d228978624
session/prompt   same session ID

schema_retry attempt=2
session/resume   same session ID
session/prompt   same session ID

schema_retry attempt=3
session/resume   same session ID
session/prompt   same session ID
```

因此“重复失败是因为没有复用 session”这一假设不成立。session reuse 已经工作，错误对象在每次 retry 中仍然被错误地交给 scheduler。

### 3.3 失败终态

最终 agent 事件为：

```text
agent:       compare:health.get
status:      Error
elapsed_ms:  1222101
tokens:      0
schema_retry: 3
```

没有发现 MCP server 返回的 transport error。失败发生在 MCP 调用结果返回 Luft 后的业务 schema 校验阶段。

## 4. 修复方案

### 4.1 最小修复

在 `result_collector` 进入 `AgentResult` 前提取 `input`：

```rust
fn normalize_structured_output(value: serde_json::Value) -> serde_json::Value {
    if let Some(input) = value
        .get("input")
        .filter(|_| value.get("schema").is_some())
    {
        return input.clone();
    }
    value
}
```

然后：

```rust
let output = workflow_validate_schema
    .map(normalize_structured_output)
    .unwrap_or_else(|| /* existing fallback */);
```

`schema` 只用于 MCP server 本次调用的校验，不能进入最终业务结果。

### 4.2 不应采用的修复

不应要求模型把业务结果改成：

```json
{
  "result": { "...": "..." }
}
```

除非同时修改 MCP tool 的 Rust 参数定义。当前 tool schema 要求的是 `input` 和 `schema`；仅修改 Lua prompt 会让模型与实际 MCP input schema 不一致。

也不应在 scheduler 中把 `PLAN_SCHEMA` 改成允许 `input` wrapper。那会把 transport 层协议泄漏到 workflow 业务契约，并使其他 backend 的直接业务对象无法统一校验。

### 4.3 推荐的分层边界

```text
MCP tool arguments: { input, schema }
        |
        | MCP server: validate(input, schema)
        v
ACP rawInput:       { input, schema }
        |
        | adapter normalization: extract input
        v
AgentResult.output: <business result>
        |
        | scheduler: validate(output, task.output_schema)
        v
workflow agent output
```

## 5. 测试计划

| 测试 | 验证点 |
|---|---|
| `normalize_direct_business_object` | 没有 wrapper 的业务对象保持不变 |
| `normalize_mcp_input_schema_wrapper` | `{ input, schema }` 只提取 `input` |
| `normalize_does_not_return_schema` | `schema` 不出现在最终 output |
| `collector_output_matches_plan_schema` | 当前 `health.get` payload 经采集后可通过 `PLAN_SCHEMA` |
| `mcp_tool_validates_input_against_schema` | MCP server 仍对 `input` 使用调用参数中的 `schema` 校验 |
| `schema_retry_reuses_session` | schema retry 使用同一个 ACP session ID |
| `repeated_wrapper_error_is_diagnosed` | 连续相同 wrapper 错误可输出明确诊断，而不是无限重试 |

建议至少执行：

```text
cargo test -p luft-adapters --lib
cargo test -p luft-cli --lib mcp_server
cargo test -p luft-cli --test workflow_validate_schema_flow
cargo test -p luft-core --lib scheduler
cargo check --workspace
```

## 6. 文档一致性问题

`docs/design/agent-session-schema-retry.md` 中部分段落仍描述旧契约 `{ "result": ... }`。该文档记录的是早期设计，应在实现修复后同步更新为当前实际契约：

```json
{
  "input": <business result>,
  "schema": <JSON Schema>
}
```

本调查文档以当前 `crates/luft-cli/src/commands/mcp_server.rs` 源码为准。
