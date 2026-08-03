# Agent Session Resume 与结构化结果校验

> **状态**：已实现（ACP resume、checkpoint 元数据和 schema retry 已接入）
>
> **目标**：为 `agent()` 增加可恢复的 agent session，使 schema 校验失败后可以复用原会话进行纠正提交，同时修复工具调用包络与业务结果 schema 混淆导致的重复失败。
>
> **相关代码**：`crates/luft-runtime/src/sdk/task.rs`、`crates/luft-core/src/scheduler/mod.rs`、`crates/luft-adapters/src/acp_adapter.rs`
>
> **交叉参考**：[backend-per-request.md](./backend-per-request.md)、[resume-failed-cancelled.md](./resume-failed-cancelled.md)、[../architecture/runtime.md](../architecture/runtime.md)

---

## 1. 背景与问题

### 1.1 当前失败模式

启用 `schema` 后，`agent()` 会在 prompt 中要求 agent 调用 `workflow_validate_schema`。当前 scheduler 在收到结果后执行 schema 校验；失败时修改 `task.prompt` 并再次调用 backend：

```text
agent()
  -> backend.run(task)
  -> ACP session/new
  -> ACP session/prompt
  -> workflow_validate_schema
  -> schema validation failed
  -> 修改 prompt
  -> backend.run(task)       // 当前会重新创建 ACP session
  -> ACP session/new
  -> ACP session/prompt
```

当前重试存在三个问题：

1. **上下文丢失**：每次 `backend.run()` 都是 one-shot ACP session，重试 agent 需要重新读取文件和重新分析。
2. **协议错误被重复放大**：本次失败中业务对象本身已经包含全部 required 字段，但工具调用 envelope 中的 `arguments/server/tool` 被当成业务结果校验，重复重试不会改变错误边界。
3. **历史实现中的 session 标识没有贯通**：本设计用于补齐 `AgentTask`、`AgentResult`、scheduler 和 ACP adapter 之间的 session 传递。

### 1.2 代码证据

| 位置 | 当前行为 |
|---|---|
| `crates/luft-runtime/src/sdk/task.rs` | `agent()` 从 Lua opts 读取 `session_id` 并写入 `AgentTask` |
| `crates/luft-core/src/contract/backend.rs` | 已有 `AgentTask.session_id` 和 `AgentResult.session_id`，语义是跨进程 conversation resume |
| `crates/luft-core/src/scheduler/mod.rs` | schema 失败后修改 prompt，再次调用 `backend.run()` |
| `crates/luft-adapters/src/acp_adapter.rs` | 根据 capability 选择 `session/new` 或 `session/resume`，resume 失败时回退到 `session/new` |
| `crates/luft-adapters/src/result_collector.rs` | 若捕获结构化值，则直接作为最终 `AgentResult.output`，不自动剥离任意调用 envelope |

### 1.3 目标与非目标

目标：

- 为 Lua `agent()` 提供可选的 `session_id` 输入。
- 让 schema 失败后的纠正 prompt 复用同一会话上下文。
- 将工具调用参数、ACP session 元数据和业务结果分层处理。
- 让 retry 反馈明确指出包络错误，而不是只重复列出缺失字段。
- 保持不支持 session resume 的 backend 可以继续工作。

非目标：

- 不把一个 ACP 子进程永久保留为 daemon 级共享 session。
- 不让不同 workflow 或不同 agent 默认共享会话。
- 不用 session resume 替代 schema 校验或结果归一化。
- 不改变 `workflow_validate_schema` 对外的 MCP 参数契约：工具输入仍为 `{ "result": <JSON> }`。

---

## 2. 设计决策

| 维度 | 决定 | 说明 |
|---|---|---|
| Lua 参数名称 | 支持 `session_id` | 对 workflow 作者和核心合约保持一致 |
| session 所有权 | 一个 `agent()` 调用链独占一个 session | 避免并发 agent 互相污染上下文 |
| retry 方式 | schema 失败优先在同一 session 中发送纠正 prompt | 保留文件读取、分析和上一次提交上下文 |
| session 关闭 | 成功提交、取消、不可恢复错误后关闭 | 失败修复期间保持；终态后释放资源 |
| schema 校验层 | 对业务结果校验，不对 MCP/ACP envelope 校验 | 必须先提取 `result` 并去除传输元数据 |
| retry 上限 | 默认最多 2 次 schema 修复 | 防止同一个错误 session 无限消耗 token |
| 不支持 resume 的 backend | 回退为创建新 session | 通过 capability 判断，不破坏既有 backend |
| 持久化 | checkpoint 保存 session 元数据；首期仅运行内复用 | 保存用于诊断和进程内 registry 对账，不直接承诺 ACP session ID 长期有效 |

---

## 3. 总体架构

```text
Lua workflow
  |
  | agent({ prompt, schema, session_id? })
  v
Runtime SDK
  | build_task(): session_id -> AgentTask.session_id
  v
Scheduler
  | 1. 调 backend.run(task)
  | 2. normalize structured submission
  | 3. validate business result
  | 4. schema failure -> corrective retry
  v
AgentBackend
  | supports_session_resume?
  | resume existing thread/session or create fresh session
  v
ACP adapter
  | initialize
  | session/new OR session/resume
  | session/prompt
  | return AgentResult { output, session_id }
```

关键分层：

```text
MCP wire request:       { result: <business-result> }
ACP tool-call metadata: { server, tool, arguments }
Luft result:             <business-result>
Schema validator:       只接收 <business-result>
```

如果某个 backend 返回的是完整工具调用 envelope，必须在 adapter 或 scheduler 的统一归一化层处理，不能把 envelope 直接送入 JSON Schema validator。

---

## 4. API 设计

### 4.1 Lua `agent()` 参数

```lua
local result = agent({
    name = "compare:health.get",
    prompt = compare_prompt,
    schema = PLAN_SCHEMA,
    session_id = nil,
    max_schema_retries = 2,
})
```

字段定义：

| 字段 | 类型 | 必填 | 说明 |
|---|---|---:|---|
| `prompt` | string | 是 | 首次任务或同 session 的纠正 prompt |
| `schema` | table/string | 否 | 业务结果 JSON Schema |
| `session_id` | string | 否 | 复用已有 agent conversation |
| `max_schema_retries` | integer | 否 | 覆盖默认 schema 修复次数；不得超过 runtime 上限 |
| `timeout_ms` | integer | 否 | 每次 session 调用的 idle timeout |

`session_id` 只能复用同一个 workflow run 中由 Luft 返回的 session。外部任意字符串必须经过 backend 验证，不能直接当作 ACP session ID 使用。

### 4.2 Lua 返回值

```lua
{
    ok = true,
    status = "ok",
    output = { ... },
    session_id = "luft-session-id",
    tokens = 1234,
    findings = {},
    attempts = 1,
    schema_retries = 0,
}
```

失败时也返回 session 信息（如果 backend 提供）：

```lua
{
    ok = false,
    status = "error",
    output = {
        error = "schema validation failed",
        validation = {
            path = "",
            message = "...",
        },
    },
    session_id = "...",
    attempts = 3,
    schema_retries = 2,
}
```

### 4.3 Rust 核心合约

首期尽量复用现有字段，不新增第二套 `conversation_id`：

```rust
pub struct AgentTask {
    // existing fields...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub max_schema_retries: Option<u32>,
}

pub struct AgentResult {
    // existing fields...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub attempts: u32,
    pub schema_retries: u32,
}
```

如果为了最小改动不扩展 `AgentTask`，`max_schema_retries` 可以暂时放入 scheduler 配置；但 `session_id` 必须贯通 task/result，否则 Lua 无法把第一次会话交给后续校验 agent。

---

## 5. Session 生命周期

### 5.1 首次调用

```text
agent({ session_id = nil })
  -> backend 创建 ACP session
  -> backend 返回 AgentResult.session_id
  -> SDK 映射为 result.session_id
```

### 5.2 schema 失败后的同会话修复

```text
backend 返回 business_result
  -> normalize
  -> validate(schema) = error
  -> 保留 result.session_id
  -> 构造 corrective prompt
  -> backend.run(AgentTask { session_id = old_session_id })
  -> session resume / conversation continue
  -> 再次捕获 workflow_validate_schema
```

纠正 prompt 必须包含：

1. 具体 validator error path。
2. 上一次归一化后的业务结果。
3. 期望的工具参数格式。
4. 明确禁止的 envelope 格式。
5. “不要重新分析文件，只修正提交格式”的指令。

示例：

```text
上一次分析结果内容基本完整，但提交格式不正确。

请在当前会话中重新提交，不要重新读取或分析文件。

工具调用参数必须是：
{"result": {<业务结果对象>}}

业务结果对象的顶层必须直接包含：
endpoint, opencode_files, loom_files, evidence, differences,
changes, acceptance, out_of_scope, summary

不要把业务字段放在 arguments 中，也不要自行添加 server 或 tool 字段。
提交后不要返回解释性文本。
```

### 5.3 终态处理

| 终态 | session 行为 | 返回行为 |
|---|---|---|
| schema 成功 | 关闭/回收 session | `ok=true`，返回业务对象 |
| 达到 schema retry 上限 | 关闭 session | `ok=false`，返回最后一次结果和校验错误 |
| backend 不支持 resume | 关闭旧 session，创建新 session | 在 prompt 中携带最小必要上下文 |
| 用户取消 | 立即取消 ACP 子进程 | `status=cancelled` |
| idle timeout | 关闭 session | `status=timed_out`，保留 session ID 供诊断但默认不可继续 |
| workflow 进程崩溃 | 由 checkpoint 保存 session ID 和恢复元数据 | 后续是否恢复取决于 backend 能否跨进程 resume |

---

## 6. Backend 接口与 ACP 实现

### 6.1 Capability

扩展 `AgentCapabilities`：

```rust
pub struct AgentCapabilities {
    pub streaming: bool,
    pub mcp_injection: bool,
    pub workflow_validate_schema: bool,
    pub session_resume: bool,
    pub models: Vec<String>,
}
```

只有 `session_resume=true` 时，scheduler 才允许把 `session_id` 传给 backend。

### 6.2 AgentBackend trait

保留现有 `run()` 作为兼容入口，增加可选的 resume 能力：

```rust
#[async_trait]
pub trait AgentBackend: Send + Sync {
    async fn run(&self, task: AgentTask, ctx: RunContext)
        -> Result<AgentResult, BackendError>;

    async fn resume(&self, task: AgentTask, ctx: RunContext)
        -> Result<AgentResult, BackendError> {
        self.run(task, ctx).await
    }
}
```

更稳妥的实现方式是增加 `run_mode` 或统一在 `run()` 内根据 `task.session_id` 分支，避免所有第三方 backend 立刻实现新 trait 方法：

```rust
match (&task.session_id, backend.capabilities().session_resume) {
    (Some(_), true) => backend.resume(task, ctx).await,
    (Some(_), false) => backend.run(task, ctx).await,
    (None, _) => backend.run(task, ctx).await,
}
```

### 6.3 ACP adapter

当前 ACP adapter 的 `run()` 是一次性流程：

```text
spawn child
-> initialize
-> session/new
-> session/prompt
-> collect result
-> kill/wait child
```

要支持真正的同 session resume，需要引入 `AcpConversation` 运行时对象，而不能只把字符串 `session_id` 传给已经退出的子进程：

```rust
struct AcpConversation {
    backend_id: String,
    process: ChildHandle,
    protocol_session_id: String,
    workdir: PathBuf,
    schema_mcp: Option<SchemaMcpHandle>,
}
```

推荐分两期实现：

**Phase A：同一次 `backend.run()` 内恢复**

- 将 schema retry 从 scheduler 的外层 `backend.run()` 循环下沉到 ACP adapter。
- ACP 子进程只 spawn 一次。
- 首次 `session/prompt` 返回错误后，在同一个 protocol session 上再次发送 `session/prompt`。
- 成功或最终失败后关闭子进程。

**Phase B：跨 `agent()` 调用恢复**

- runtime 保存活跃 `AcpConversation` 的句柄。
- `AgentResult.session_id` 返回一个 Luft-owned opaque ID，而不是裸 ACP session ID。
- 后续 `agent({session_id=...})` 通过 registry 查找 conversation。
- workflow 结束、取消或 session 超时后删除 registry 项。

不建议直接把 ACP 原始 session ID 暴露给 Lua，因为 ACP 子进程生命周期和 session ID 有一一对应关系，子进程退出后这个 ID 通常不能独立恢复。

---

## 7. Structured Output 归一化

### 7.1 统一输入形式

`workflow_validate_schema` 的 MCP 工具参数仍然是：

```json
{
  "result": {
    "endpoint": "GET /global/health",
    "summary": "..."
  }
}
```

归一化函数负责把不同 transport 层表示转换为业务对象：

```rust
fn normalize_structured_submission(value: Value) -> Result<Value, NormalizeError> {
    // 1. MCP request wrapper: { result: business }
    // 2. ACP event wrapper: { arguments: { result: business }, server, tool }
    // 3. 兼容旧实现：{ arguments: business, server, tool }
    // 4. 最终返回 business object
}
```

### 7.2 归一化规则

| 输入 | 处理 |
|---|---|
| `{result: object}` | 返回 `result` |
| `{arguments: {result: object}, server, tool}` | 返回 `arguments.result` |
| `{arguments: object, server, tool}` | 兼容性剥离后返回 `arguments`，并记录 warning |
| 直接业务对象 | 原样返回 |
| `result` 不是 object，但 schema 允许 scalar | 按 schema 决定是否接受 |
| 多层 wrapper | 最多递归 2 层，超过则报明确错误 |

归一化必须发生在 `validate_output()` 之前，并且最终 `AgentResult.output` 保存归一化后的业务对象，不能保存 transport envelope。

### 7.3 诊断日志

schema 失败时记录结构化字段：

```text
raw_output_shape = "acp_tool_envelope"
normalized_output_shape = "object"
unwrapped_paths = ["arguments", "result"]
validation_path = ""
validation_error = "required property: endpoint"
```

日志中不应默认记录完整 prompt 或敏感业务字段。

---

## 8. Scheduler Retry 改造

### 8.1 当前问题

当前 scheduler 的 schema retry 会修改同一个 `task.prompt`，但每次循环都重新执行 `backend.run(task)`。因此“retry”实际上是“新 session 重跑”。

### 8.2 目标逻辑

```rust
let mut session_id = task.session_id.clone();
let mut retries = 0;

loop {
    let result = run_task_with_session(&backend, task.clone(), session_id.clone(), ctx).await?;
    let normalized = normalize_structured_submission(result.output)?;

    match validate_output(&normalized, schema) {
        Ok(()) => return Ok(result.with_output(normalized)),
        Err(error) if retries < max_schema_retries => {
            retries += 1;
            session_id = result.session_id.clone();
            task.prompt = corrective_prompt(
                &original_prompt,
                &normalized,
                &error,
                retries,
            );
        }
        Err(error) => return Err(SchedulerError::SchemaValidation(error.to_string())),
    }
}
```

### 8.3 Retry 分类

| 错误类型 | 是否同 session retry | 处理 |
|---|---:|---|
| schema required property 缺失 | 是 | 给出缺失字段和正确工具格式 |
| envelope 包络错误 | 是 | 只要求重新提交，不重新分析 |
| 返回纯文本 | 是 | 明确禁止文本，要求调用工具 |
| backend spawn failed | 否 | 使用 backend retry 或直接失败 |
| ACP connection closed | 否/条件 | 若 session 可恢复则 resume，否则新 session |
| timeout | 否 | 不在旧 session 上盲目重试，报告 session 状态 |
| 用户取消 | 否 | 立即终止 |

连续两次检测到同一个 `raw_output_shape + validation_error` 时，应停止 retry，并返回“重复协议错误”诊断。

---

## 9. Workflow 使用方式

### 9.1 显式校验 agent

```lua
local compare = agent({
    name = "compare:" .. endpoint.id,
    prompt = compare_prompt(endpoint),
    schema = PLAN_SCHEMA,
    max_schema_retries = 2,
})

if not compare.ok then
    log("compare failed: " .. json.encode(compare.output), "error")
    report({
        status = "failed",
        endpoint = endpoint.id,
        error = compare.output,
    })
    return
end

local verify = agent({
    name = "verify:" .. endpoint.id,
    session_id = compare.session_id,
    prompt = verify_prompt,
    schema = VERIFY_SCHEMA,
})
```

### 9.2 不建议的使用方式

```lua
-- 不建议：把同一个 session_id 传给并发的多个 agent
parallel(items, function(item)
    return agent({ session_id = shared_session, prompt = item.prompt })
end)
```

同一个 session 必须串行使用；并发复用会导致 prompt、工具调用和结果相互污染。

### 9.3 Compare/Test/Develop 生命周期

对于当前 opencode/loom endpoint 工作流，建议默认使用不同 session：

```text
compare session  -> 只负责源码比较和计划
test session     -> 独立测试设计/执行
develop session  -> 允许修改文件
adversarial      -> 独立 agent，避免自我验证
```

只有以下情况才复用 session：

- compare 的 schema 修复；
- test agent 对自己刚生成的测试结果进行格式纠正；
- develop agent 因工具调用或结构化提交失败而重试。

对抗性验证必须保持独立 session，不能使用 develop agent 的 session，否则会削弱独立性。

---

## 10. 持久化与跨 workflow resume

### 10.1 首期范围

首期只支持单次 workflow 运行期间的 session resume：

- `session_id` 不写入外部用户可控配置；
- 写入 agent journal/checkpoint，供当前进程诊断、崩溃后的状态判断和 registry 对账；
- workflow 结束后 session registry 清理；
- `resume_from_id` 不保证继续使用已经退出的 ACP 子进程。

checkpoint 在 `agent_sessions` 中保存每个 agent 的 session 元数据：

```rust
pub struct AgentSessionCheckpoint {
    pub agent_id: AgentId,
    pub session_id: String,              // logical Luft session ID
    pub backend_id: Option<String>,
    pub protocol_session_id: Option<String>,
    pub status: SessionCheckpointStatus,
    pub updated_at: u64,
    pub resumable: bool,
}
```

`RunCheckpoint` 增加：

```rust
#[serde(default)]
pub agent_sessions: HashMap<AgentId, AgentSessionCheckpoint>,
```

写入规则：

- backend 创建或恢复 session 后，收到 `AgentResult.session_id` 时 upsert；
- schema retry 每次沿用同一个 `session_id`，更新 `updated_at` 和状态；
- 成功、取消、不可恢复错误或超时后标记终态，再由 workflow cleanup 删除 registry；
- checkpoint 中保存 `session_id` 不等于保存 ACP conversation 本身；只有 backend 提供跨进程恢复协议时，`resumable=true` 才有效。

### 10.2 后续跨进程恢复

如果未来要支持 `resume_from_id` 后继续 conversation，需要持久化：

```text
session_id
backend_id
protocol_session_id 或 backend-native checkpoint
workdir
model
schema hash
workflow run_id
agent_id
session state
```

必须确认 backend 提供真正的跨进程 resume 能力。仅保存 ACP `session_id` 不足以恢复已退出的 ACP 子进程。

---

## 11. 改动文件清单

| 文件 | 改动类型 | 说明 |
|---|---|---|
| `crates/luft-runtime/src/sdk/task.rs` | 修改 | 读取 `session_id`、`max_schema_retries`，写入 `AgentTask.session_id` |
| `crates/luft-runtime/src/sdk/agent/single.rs` | 修改 | 暴露 `session_id`，记录 retry 元数据 |
| `crates/luft-runtime/src/sdk/agent/parallel.rs` | 修改 | 传递 session 信息，禁止多个并发任务共享同一 session |
| `crates/luft-runtime/src/sdk/agent/pmap.rs` | 修改 | 与 parallel 路径保持一致 |
| `crates/luft-core/src/contract/backend.rs` | 修改 | 增加 `session_resume` capability 和 retry 元数据 |
| `crates/luft-core/src/scheduler/mod.rs` | 修改 | session-aware schema retry、错误分类、重复错误熔断 |
| `crates/luft-adapters/src/acp_adapter.rs` | 修改 | 抽取可复用 ACP conversation，返回/复用 session ID |
| `crates/luft-adapters/src/result_collector.rs` | 修改 | 增加 structured submission normalization |
| `crates/luft-core/src/contract/event.rs` | 修改 | 增加 session/retry/normalization 诊断字段（如确有需要） |
| `crates/luft-runtime/src/sdk/agent/journal.rs` | 修改 | 保存 session ID 和 retry 状态 |
| `crates/luft-skills/src/skill/references/primitives.md` | 修改 | 更新 `agent()` 参数和返回值文档 |
| `crates/luft-skills/src/skill/references/agent-prompts.md` | 修改 | 增加精确的 `workflow_validate_schema` 调用示例 |
| `crates/luft-adapters/tests/` | 新增/修改 | ACP session resume 与 envelope normalization 测试 |
| `crates/luft-core/tests/` | 新增/修改 | scheduler retry、capability fallback 测试 |

---

## 12. 实施阶段

### Phase 1：先修复当前 schema 错误，不改变 session 生命周期

目标：立即消除本次重复失败。

| 任务 | 产出 |
|---|---|
| 增加 `normalize_structured_submission()` | `{result}`、`{arguments}`、ACP envelope 可归一化 |
| 修改 scheduler retry prompt | 明确工具参数格式和禁止字段 |
| 增加 raw/normalized shape 日志 | 能区分内容错误与包络错误 |
| 增加重复错误熔断 | 相同错误不超过 2 次 |
| 添加单元测试 | 覆盖本次失败 payload |

### Phase 2：同一 scheduler 调用内复用 ACP session

目标：schema retry 不重新 spawn agent。

| 任务 | 产出 |
|---|---|
| 抽取 ACP conversation 状态 | 子进程和 protocol session 可持续到 retry 结束 |
| `session/prompt` 二次发送 | 同一 ACP session 继续对话 |
| `AgentResult.session_id` 回填 | scheduler 可继续使用 session |
| 增加 capability fallback | 不支持 resume 的 backend 仍走旧路径 |

### Phase 3：Lua `agent({session_id=...})`

目标：workflow 可以显式把同一个 agent session 交给后续纠正任务。

| 任务 | 产出 |
|---|---|
| 读取 `session_id` | 映射到 `AgentTask.session_id` |
| 返回 `session_id` | Lua 可保存并传给后续 agent |
| registry 生命周期 | 运行内查找和释放 conversation |
| 并发保护 | 同一 session 禁止并发 prompt |

### Phase 4：跨 workflow resume

目标：`resume_from_id` 可以在进程重启后恢复支持该能力的 backend session。

这阶段必须在确认 Loom/ACP backend 的持久化协议后实施，不应只保存裸 session ID。

---

## 13. 测试计划

### 13.1 Schema normalization

| 测试 | 验证点 |
|---|---|
| direct business object | 直接业务对象原样通过 |
| MCP `{result: object}` | 提取 `result` 后校验 |
| ACP `{arguments: {result: object}}` | 提取 `arguments.result` |
| legacy `{arguments: object}` | 兼容处理并记录 warning |
| scalar result | 由 schema 决定是否允许 |
| malformed nested wrapper | 返回可诊断错误，不 panic |
| current health payload | 复现并验证当前失败样例可以通过 |

### 13.2 Scheduler retry

| 测试 | 验证点 |
|---|---|
| schema failure then success | 第二次使用 corrective prompt 成功 |
| same-session retry | backend 收到同一个 session ID |
| retry limit | 达到上限后返回 schema error |
| repeated envelope error | 相同错误触发熔断 |
| plain text fallback | 明确提示调用工具，不无限重试 |
| non-resumable backend | 自动回退到新 session |
| cancellation during retry | 不产生孤儿 session |

### 13.3 ACP adapter

| 测试 | 验证点 |
|---|---|
| one process, two prompts | schema retry 不重复 spawn 子进程 |
| session ID propagation | `AgentResult.session_id` 与 ACP session 对应 |
| post-submission close | 成功提交后仍按 post-submission timeout 回收 |
| resume failure | backend 拒绝 resume 时返回明确错误 |
| cleanup | 成功、失败、取消、超时均释放 child/process |

### 13.4 Workflow integration

| 测试 | 验证点 |
|---|---|
| compare schema repair | compare agent 能在同 session 修复提交 |
| test/develop isolation | test 和 develop 不共享 session |
| adversarial independence | adversarial agent 使用独立 session |
| serial session reuse | 同一 endpoint 的纠正调用按顺序执行 |
| concurrency=1 | session registry 不发生交叉使用 |

---

## 14. 向后兼容性

| 场景 | 行为 |
|---|---|
| 不传 `session_id` | 与当前行为一致，创建新 session |
| backend 不支持 thread resume | 自动回退到新 session |
| 不使用 schema | 不进入 schema retry，行为不变 |
| 旧 backend 实现 `AgentBackend` | 继续调用 `run()` |
| 旧 checkpoint 无 session ID | 正常恢复，但不能继续旧 conversation |
| 旧 Lua workflow | 无需修改 |
| 外部传入未知 session ID | 返回 invalid session，不直接注入 backend |

---

## 15. 风险与缓解

### 15.1 ACP session 实际上不可 resume

部分 ACP backend 只支持当前进程内的 protocol session，进程退出后裸 session ID 无效。

**缓解**：首期只承诺同一 adapter invocation 内 resume；对外暴露 Luft-owned opaque ID；跨进程 resume 延后。

### 15.2 session 泄漏

失败后保留 session 可能导致 ACP 子进程或 MCP server 未释放。

**缓解**：为 session registry 增加 owner run ID、last activity、absolute deadline；workflow 终态统一清理；所有 cleanup 使用 RAII/Drop 或 finally 路径。

### 15.3 上下文污染

把 compare session 交给 develop 或 adversarial agent，可能使独立验证失效。

**缓解**：session 只用于同一 agent 的 schema 修复；跨角色复用需要 workflow 显式声明，并默认禁止 adversarial 复用。

### 15.4 重试成本仍然过高

同 session 虽然减少文件读取，但第二次 prompt 仍消耗 token。

**缓解**：只对协议/格式错误进行 session retry；内容缺失和业务错误交由独立 validator；默认上限 2 次。

### 15.5 schema 归一化掩盖真实 bug

兼容剥离 `{arguments: object}` 可能把错误调用“悄悄接受”。

**缓解**：兼容路径仅作为过渡；记录 warning 和 normalization path；未来版本只接受标准 `{input: object, schema: object}`。

---

## 16. 验收标准

实现完成后，以下条件必须同时满足：

1. 当前 `GET /global/health` 失败样例可以通过 schema 校验，且 `AgentResult.output` 是业务对象，不包含 `server/tool/arguments` 元数据。
2. schema retry 不再无条件创建新的 ACP session。
3. Lua `agent()` 能读取可选 `session_id`，并在返回值中提供 session ID。
4. 同一 session 的 retry 是串行的，不能被 `parallel()` 并发复用。
5. 连续相同 envelope 错误最多重试 2 次后停止。
6. 不支持 session resume 的 backend 仍可以运行旧 workflow。
7. compare、test、develop、adversarial 默认保持 session 隔离。
8. 成功、失败、取消、超时后没有残留 ACP 子进程或 session registry 条目。
9. 旧 Lua workflow 和旧 checkpoint 不需要迁移即可运行。

---

## 17. 最终建议

`session_id` 应该加入 `agent()`，但实现顺序必须是：

```text
先修复 structured-output normalization
  -> 再把 schema retry 下沉到同一个 ACP session
  -> 再开放 Lua session_id
  -> 最后考虑跨 workflow 持久化
```

如果直接先增加 `session_id`，模型仍会在错误的 `arguments/server/tool` 包络中提交，结果只是“在同一个 session 中重复失败”。真正需要建立的是可恢复的 agent conversation，以及明确分离 transport envelope 和业务结果的校验边界。
