# Agent Artifact Report Writer — 实现设计

> **状态**: 📝 设计阶段（待实现）
> **相关代码**: [`src/core/contract/event.rs`](../../src/core/contract/event.rs)、[`src/core/scheduler/mod.rs`](../../src/core/scheduler/mod.rs)、[`src/commands/run.rs`](../../src/commands/run.rs)
> **交叉参考**: [`sdk-events.md`](sdk-events.md)（事件总线机制）、[`event-logging.md`](event-logging.md)（事件日志）

---

## 0. 目标

在 agent 执行完成后，自动将其报告（含 token、轮数、工具调用数）以 Markdown 格式写入磁盘文件。方案采用**事件驱动**（方案 C）——新增一个 event consumer 订阅现有 broadcast 总线，在 agent 生命周期事件到达时生成并写入 Markdown 报告。

### 核心原则

- **零侵入**：不修改 `agent()` / `pipeline()` / `parallel()` 的 Lua 接口和 scheduler 逻辑
- **事件驱动**：与 PhaseRenderer、EventLogger、StorageWriter 平级，作为独立 consumer 挂在 broadcast channel 上
- **默认开启**：每次 `luft run` 自动写入，用 `--no-artifacts` 关闭

---

## 1. 背景：现有事件流

### 1.1 broadcast 总线

```
RunContext.events (broadcast::Sender<AgentEvent>, capacity=2048)
    │
    ├── PhaseRenderer       (CLI 实时渲染 phase 树)
    ├── EventLogger         (可选：写 events.jsonl)
    ├── StorageWriter       (持久化到 SQLite)
    └── [NEW] ArtifactWriter ← 本方案新增
```

### 1.2 agent 执行事件序列

```
RunStarted
  PhaseStarted(label="discover subsystems")
    AgentStarted(agent_id=A0, model="claude-...", prompt_preview="...")
      AgentProgress(A0, Message)          ← 每条 LLM 消息
      AgentProgress(A0, ToolCall{name, summary})  ← 每次工具调用
      AgentProgress(A0, FileEdit{path})   ← 每次文件编辑
      AgentProgress(A0, Tokens{usage})    ← token 增量
    AgentDone(A0, status=ok, tokens, elapsed_ms)
  PhaseDone(ok=1, failed=0)
RunDone(status, total_tokens, report)
```

### 1.3 pipeline 执行事件序列

```
PipelineStarted(total_stages=2, items=3)
  PipelineStageStarted(stage=0, label="analyze", agents=3)
    AgentStarted(A0) → [AgentProgress...] → AgentDone(A0)   ← item 0, stage 0
    AgentStarted(A1) → [AgentProgress...] → AgentDone(A1)   ← item 1, stage 0
    AgentStarted(A2) → [AgentProgress...] → AgentDone(A2)   ← item 2, stage 0
  PipelineStageStarted(stage=1, label="assess", agents=3)
    AgentStarted(A3) → [AgentProgress...] → AgentDone(A3)   ← item 0, stage 1
    AgentStarted(A4) → [AgentProgress...] → AgentDone(A4)   ← item 1, stage 1
    AgentStarted(A5) → [AgentProgress...] → AgentDone(A5)   ← item 2, stage 1
PipelineDone(stages_completed=2, total_ok=2, total_failed=1)
```

### 1.4 parallel 执行事件序列

```
ParallelStarted(phase_id, span_id, count=4)
    AgentStarted(A0) → AgentDone(A0)   ┐
    AgentStarted(A1) → AgentDone(A1)   │  并发，顺序不保证
    AgentStarted(A2) → AgentDone(A2)   │
    AgentStarted(A3) → AgentDone(A3)   ┘
ParallelDone(ok=3, failed=1, results=[...], elapsed_ms=15200)
```

---

## 2. 数据来源分析

### 2.1 Markdown 字段 → 事件映射

| Markdown 字段 | 来源事件 | 可用性 |
|---------------|----------|--------|
| agent_seq | `AgentDone.agent_seq` | ❌ **需新增** |
| name | `AgentDone.name` | ❌ **需新增** |
| agent_id | `AgentDone.agent_id` | ✅ |
| status | `AgentDone.status` | ✅ |
| elapsed_ms | `AgentDone.elapsed_ms` | ✅ |
| tokens (input/output/cache) | `AgentDone.tokens: TokenUsage` | ✅ |
| model | `AgentStarted.model` | ✅ |
| phase label | `AgentStarted.phase_id` → 查找最近 `PhaseStarted.label` | ✅ |
| pipeline stage/item | `PipelineStageStarted.stage_index` + `PipelineItemDone.item_index` | ✅ |
| **轮数 (rounds)** | 累计 `AgentProgress::Message` 计数 | ✅ |
| **工具调用数** | 累计 `AgentProgress::ToolCall` 计数 | ✅ |
| **工具调用明细** | `AgentProgress::ToolCall{name, summary}` 分组计数 | ✅ |
| **文件编辑数** | 累计 `AgentProgress::FileEdit` 计数 | ✅ |
| **agent output** | `AgentDone.output` | ❌ **当前事件不携带** |
| **findings** | `AgentDone.findings` | ❌ **当前事件不携带** |

### 2.2 关键阻塞：AgentDone 缺 output/findings

当前 `AgentDone` 事件定义（`src/core/contract/event.rs:63-69`）：

```rust
AgentDone {
    run_id: RunId,
    agent_id: AgentId,
    status: AgentStatus,
    tokens: TokenUsage,
    elapsed_ms: u64,
    // ❌ 缺少 output 和 findings
}
```

agent 的实际 output 和 findings 在 `AgentResult`（`src/core/contract/backend.rs:59-72`）中，由 `sched.run_agent()` 返回并传入 `build_result_table` 进入 Lua——**但从不进入事件流**。

### 2.3 ProgressDelta 类型

`AgentProgress` 携带 `delta: ProgressDelta`（`src/core/contract/event.rs:197-202`）：

```rust
pub enum ProgressDelta {
    Message { text: String },           // LLM 消息 → 计为 1 轮
    ToolCall { name: String, summary: String },  // 工具调用
    FileEdit { path: PathBuf },         // 文件编辑
    Tokens { usage: TokenUsage },       // token 增量
}
```

ArtifactWriter 通过累计 `Message` 计数得到轮数，累计 `ToolCall` 分组得到工具调用明细。

---

## 3. 设计决策

| 维度 | 决定 | 理由 |
|------|------|------|
| ① 写入触发 | `AgentDone` 事件到达时写单 agent 报告 | 事件驱动，零侵入 |
| ② output 来源 | 扩充 `AgentDone` 加 `output` + `findings` + `prompt` | 新建事件类型破坏面更大；现有消费者用 `#[serde(default)]` 兼容 |
| ③ 轮数/工具数 | 累计 `AgentProgress` 事件 | 不改 SDK / scheduler，仅 consumer 端计数 |
| ④ 文件格式 | Markdown（含 JSON code block 保真 output） | 人可读 + 机可解析 |
| ⑤ 文件结构 | `{run_dir}/{run_id}/{seq}_{name}/report.md` | name 可选，无 name 时为 `{seq}` |
| ⑥ pipeline/parallel 聚合 | `PipelineDone` / `ParallelDone` 时写汇总表 | 一眼看全局 |
| ⑦ 开关 | 默认开，`--no-artifacts` 关闭 | 默认有价值，可关 |
| ⑧ PipelineItemDone token bug | 需修复（当前硬编码 `TokenUsage::default()`） | 否则聚合表 token 恒 0 |
| ⑨ agent 标识 | 新增 `name: Option<String>` + `agent_seq: u32` | UUID 不可读，seq + name 人类友好 |
| ⑩ agent_seq 计数器 | 放 SdkContext（与 `phase_counter` 同级） | Lua 侧分配，全局递增，跨 pipeline/parallel 共享 |

---

## 4. 事件扩充：AgentDone 加 output/findings + name/seq

本方案需要扩充两类事件字段：
1. **AgentDone** 加 `output` / `findings` / `prompt`（§4.1-4.3）
2. **AgentStarted + AgentDone** 加 `name` / `agent_seq`（§4.4-4.6）

### 4.1 改动 `AgentEvent::AgentDone` — output/findings/prompt

```rust
// src/core/contract/event.rs
AgentDone {
    run_id: RunId,
    agent_id: AgentId,
    status: AgentStatus,
    tokens: TokenUsage,
    elapsed_ms: u64,
    #[serde(default)]
    name: Option<String>,          // 新增：agent 短标签
    #[serde(default)]
    agent_seq: u32,                // 新增：全局单调递增序号
    #[serde(default)]
    output: serde_json::Value,     // 新增：agent 的结构化输出
    #[serde(default)]
    findings: Vec<Finding>,        // 新增：agent 的 findings
    #[serde(default)]
    prompt: String,                // 新增：完整 prompt（用于报告）
},
```

### 4.2 改动 `AgentEvent::AgentStarted` — name/seq

```rust
// src/core/contract/event.rs
AgentStarted {
    run_id: RunId,
    phase_id: PhaseId,
    agent_id: AgentId,
    prompt_preview: String,
    model: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    name: Option<String>,          // 新增
    #[serde(default)]
    agent_seq: u32,                // 新增
},
```

### 4.3 改动发射点

`src/core/scheduler/mod.rs:294-300`：

```rust
let _ = events.send(AgentEvent::AgentDone {
    run_id,
    agent_id: task.agent_id,
    status: status.clone(),
    tokens,
    elapsed_ms,
    name: task.name.clone(),              // 新增
    agent_seq: task.agent_seq,            // 新增
    output: result.output.clone(),        // 新增
    findings: result.findings.clone(),    // 新增
    prompt: task.prompt.clone(),          // 新增
});
```

AgentStarted 发射点（`src/core/scheduler/mod.rs` 中 agent 启动处）同步补上 `name` + `agent_seq`。

### 4.4 向后兼容

现有消费者（PhaseRenderer、EventLogger、StorageWriter）不需要新字段，全部用 `#[serde(default)]`：
- JSON 反序列化：旧日志（无新字段）可正常解析
- 模式匹配：使用 `..` 的 match 不受影响

**编译器兜底**：所有构造 `AgentStarted` / `AgentDone` 的地方编译时会强制补上新字段。

### 4.5 改动 `AgentTask` — name/seq

```rust
// src/core/contract/backend.rs
pub struct AgentTask {
    pub agent_id: AgentId,
    pub phase_id: PhaseId,
    pub prompt: String,
    pub model: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub name: Option<String>,       // 新增
    #[serde(default)]
    pub agent_seq: u32,             // 新增
    pub allowlist: Option<ToolPolicy>,
    pub workdir: PathBuf,
    pub mcp_endpoint: Option<McpEndpoint>,
    pub timeout: Option<Duration>,
    pub output_schema: Option<serde_json::Value>,
}
```

### 4.6 SdkContext 新增 agent_seq 计数器

```rust
// src/runtime/sdk/mod.rs (SdkContext)
pub struct SdkContext {
    pub events: broadcast::Sender<AgentEvent>,
    pub phase_counter: Arc<AtomicU32>,    // 现有
    pub agent_seq_counter: Arc<AtomicU32>, // 新增：全局 agent 序号
    // ...
}
```

`src/runtime/sdk/task.rs` — `build_task()` 中分配序号：

```rust
let agent_seq = agent_seq_counter.fetch_add(1, Ordering::Relaxed);

let task = AgentTask {
    agent_id: uuid::Uuid::now_v7(),
    phase_id,
    agent_seq,                          // 新增
    name: opts.get("name")               // 新增：从 Lua opts 读取
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string()),
    prompt,
    model,
    // ...
};
```

### 4.7 Lua API 扩展

`agent()` 函数新增可选参数 `name`：

```lua
local r = agent({
    name = "analyze_auth",       -- 新增：可选短标签（蛇形标识符）
    prompt = "Analyze the auth module...",
    schema = AUTH_SCHEMA,
})
```

`name` 与 `description` 的区别：

| 字段 | 用途 | 示例 | 格式约束 |
|------|------|------|----------|
| `name` | 短标签（目录名、表格标识） | `analyze_auth` | 蛇形，`^[a-z][a-z0-9_]*$`，≤32 字符 |
| `description` | 一句话描述（报告描述行） | `Read source files and identify key abstractions` | 自由文本 |

---

## 5. Bug 修复：PipelineItemDone token

### 5.1 现状

`src/runtime/pipeline.rs` 中 `PipelineItemDone` 的 `tokens` 字段硬编码为 `TokenUsage::default()`：

```rust
let _ = tx.send(AgentEvent::PipelineItemDone {
    run_id,
    stage_index: stage_idx,
    item_index: item.index,
    status,
    tokens: TokenUsage::default(),  // ← BUG: 永远是 0
    elapsed_ms: elapsed,
});
```

### 5.2 修复

Pipeline 内部执行 agent 时已经有 `AgentResult`，从中取 `tokens_used`：

```rust
tokens: result.tokens_used,  // 修复：传播实际 token
```

---

## 6. ArtifactWriter 设计

### 6.1 模块位置

```
src/commands/artifact_writer.rs   (新文件)
```

与 `phase_renderer.rs`、`event_log.rs` 平级。

### 6.2 结构体

```rust
/// Event consumer that writes Markdown artifact reports for each agent.
pub struct ArtifactWriter {
    /// Base directory: {runs_base_dir}/{run_dir_name}/
    base: PathBuf,
    /// {run_id} for the directory structure
    run_id: RunId,
    /// Per-agent accumulated stats (keyed by agent_id)
    agents: HashMap<AgentId, AgentStats>,
    /// Active pipeline/parallel context (for associating agents)
    pipeline_ctx: Option<PipelineContext>,
    parallel_ctx: Option<ParallelContext>,
    /// Phase labels (phase_id → label)
    phases: HashMap<PhaseId, String>,
}

struct AgentStats {
    agent_id: AgentId,
    agent_seq: u32,                    // 新增
    name: Option<String>,              // 新增
    model: Option<String>,
    phase_label: Option<String>,
    messages: u32,                    // Message delta count = rounds
    tool_calls: HashMap<String, u32>, // name → count
    file_edits: Vec<PathBuf>,
    pipeline_stage: Option<usize>,
    pipeline_item: Option<usize>,
}

struct PipelineContext {
    total_stages: usize,
    total_items: usize,
    current_stage: usize,
    stage_label: String,
    items: Vec<PipelineItemRecord>,
}

struct PipelineItemRecord {
    item_index: usize,
    stage_results: Vec<Option<AgentDoneRecord>>, // per-stage
}

struct AgentDoneRecord {
    agent_id: AgentId,
    status: AgentStatus,
    tokens: TokenUsage,
    elapsed_ms: u64,
}
```

### 6.3 事件处理

```rust
impl ArtifactWriter {
    pub fn handle(&mut self, evt: &AgentEvent) {
        match evt {
            AgentEvent::AgentStarted { agent_id, model, phase_id, .. } => {
                let stats = self.agents.entry(*agent_id).or_default();
                stats.model = model.clone();
                stats.phase_label = self.phases.get(phase_id).cloned();
                // 如果在 pipeline 上下文中，关联 stage/item
                if let Some(ctx) = &self.pipeline_ctx {
                    stats.pipeline_stage = Some(ctx.current_stage);
                }
            }
            AgentEvent::AgentProgress { agent_id, delta } => {
                if let Some(stats) = self.agents.get_mut(agent_id) {
                    match delta {
                        ProgressDelta::Message { .. } => stats.messages += 1,
                        ProgressDelta::ToolCall { name, .. } => {
                            *stats.tool_calls.entry(name.clone()).or_default() += 1;
                        }
                        ProgressDelta::FileEdit { path } => stats.file_edits.push(path.clone()),
                        ProgressDelta::Tokens { .. } => {} // 不累计，用 AgentDone.tokens
                    }
                }
            }
            AgentEvent::AgentDone { agent_id, status, tokens, elapsed_ms, output, findings, .. } => {
                let stats = self.agents.remove(agent_id).unwrap_or_default();
                let model = stats.model.clone();
                let phase_label = stats.phase_label.clone();
                let messages = stats.messages;
                let tool_calls = stats.tool_calls.clone();
                let file_edits = &stats.file_edits;
                let pipeline_stage = stats.pipeline_stage;

                self.write_agent_report(
                    agent_id, status, tokens, elapsed_ms,
                    model, phase_label, messages, &tool_calls, file_edits,
                    pipeline_stage, output, findings,
                );
            }
            AgentEvent::PhaseStarted { phase_id, label, .. } => {
                self.phases.insert(*phase_id, label.clone());
            }
            AgentEvent::PipelineStarted { total_stages, items, .. } => {
                self.pipeline_ctx = Some(PipelineContext {
                    total_stages: *total_stages,
                    total_items: *items,
                    current_stage: 0,
                    stage_label: String::new(),
                    items: (0..*items).map(|i| PipelineItemRecord {
                        item_index: i,
                        stage_results: vec![None; *total_stages],
                    }).collect(),
                });
            }
            AgentEvent::PipelineStageStarted { stage_index, label, .. } => {
                if let Some(ctx) = &mut self.pipeline_ctx {
                    ctx.current_stage = *stage_index;
                    ctx.stage_label = label.clone();
                }
            }
            AgentEvent::PipelineItemDone { stage_index, item_index, status, tokens, elapsed_ms, .. } => {
                if let Some(ctx) = &mut self.pipeline_ctx {
                    if let Some(item) = ctx.items.iter_mut().find(|i| i.item_index == *item_index) {
                        if let Some(slot) = item.stage_results.get_mut(*stage_index) {
                            *slot = Some(AgentDoneRecord {
                                agent_id: Uuid::nil(), // PipelineItemDone 不带 agent_id
                                status: status.clone(),
                                tokens: *tokens,
                                elapsed_ms: *elapsed_ms,
                            });
                        }
                    }
                }
            }
            AgentEvent::PipelineDone { stages_completed, total_ok, total_failed, .. } => {
                if let Some(ctx) = self.pipeline_ctx.take() {
                    self.write_pipeline_summary(ctx, *stages_completed, *total_ok, *total_failed);
                }
            }
            AgentEvent::ParallelStarted { span_id, count, .. } => {
                // 记录 parallel 上下文
            }
            AgentEvent::ParallelDone { ok, failed, results, elapsed_ms, .. } => {
                self.write_parallel_summary(*ok, *failed, results, *elapsed_ms);
            }
            AgentEvent::ReportEmitted { report, .. } => {
                self.write_run_report(report);
            }
            AgentEvent::RunDone { status, total_tokens, .. } => {
                self.write_run_summary(status, total_tokens);
            }
            _ => {}
        }
    }
}
```

---

## 7. Markdown 格式规范

每个报告由若干**区块 (section)** 组成，各区块有明确的数据来源和展示规则。以下按报告类型逐一说明格式、字段含义与数据映射。

---

### 7.1 单 agent 报告 `{run_id}/{seq}_{name}/report.md`

#### 完整示例

```markdown
# Agent #3 `analyze_auth`

> analyze auth — Read the source files and identify key abstractions.

## Metadata

| Field    | Value                     |
|----------|---------------------------|
| Seq      | 3                         |
| Name     | analyze_auth              |
| Agent ID | 0192...ab12               |
| Status   | ok                        |
| Model    | claude-sonnet-4-20250514  |
| Phase    | analyze auth              |
| Pipeline | Stage 1/2 (assess), Item 0 |
| Elapsed  | 12.3s                     |

## Token Usage

| Metric      | Count |
|-------------|-------|
| Input       | 1,234 |
| Output      |   567 |
| Cache Read  |   890 |
| Cache Write |   123 |
| **Total**   | 1,801 |

## Execution

- Rounds: 3
- Tool Calls: 7
  - `read_file`: 3
  - `grep`: 2
  - `bash`: 2
- File Edits: 1
  - `src/auth.rs`

## Prompt

```
Perform a thorough architecture analysis of this module:
Path: src/auth.rs
Module: auth
...
```

## Output

```json
{
  "module": "auth",
  "responsibility": "Handles authentication and session management",
  "key_abis": [...],
  ...
}
```

## Findings

- **High**: XSS Risk — User input not sanitized
- **Medium**: Slow Query — Database query takes >500ms
```

#### 区块说明

| 区块 | 内容 | 数据来源 | 展示规则 |
|------|------|----------|----------|
| **标题** | `# Agent #{seq} \`{name}\`` | `AgentDone.agent_seq` + `name` | name 为 None 时只显示 `# Agent #{seq}` |
| **描述引用行** | `> {phase_label} — {description}` | `PhaseStarted.label` + `AgentStarted.description` | description 为 None 时只显示 phase_label |
| **Metadata** | agent 元信息 | 见下方字段表 | 表格，无值字段显示 `-` |
| **Token Usage** | token 消耗明细 | `AgentDone.tokens: TokenUsage` | 表格；Total = input + output（不含 cache） |
| **Execution** | 执行过程统计 | 累计 `AgentProgress` 事件 | 见下方字段表 |
| **Prompt** | 发给 agent 的完整 prompt | `AgentDone.prompt`（新增字段） | code block 原文；超长时截断显示前 200 行 + `... (truncated)` |
| **Output** | agent 的结构化输出 | `AgentDone.output`（新增字段） | JSON code block；status 非 ok 时显示 error 信息 |
| **Findings** | agent 发现的问题 | `AgentDone.findings`（新增字段） | 按严重度排序：Critical → High → Medium → Low |

#### Metadata 字段

| 字段 | 含义 | 来源事件 | 备注 |
|------|------|----------|------|
| Seq | 全局序号 | `AgentDone.agent_seq` | 从 0 开始单调递增 |
| Name | agent 短标签 | `AgentDone.name` | 可选；None 时显示 `-` |
| Agent ID | UUID | `AgentDone.agent_id` | 前 12 位缩写 |
| Status | agent 结束状态 | `AgentDone.status` | ok / error / cancelled / timed_out |
| Model | LLM 模型 | `AgentStarted.model` | 可能 None（用默认模型时） |
| Phase | 当前 phase 名称 | `PhaseStarted.label`（通过 `phase_id` 关联） | 反映 agent 所属的工作阶段 |
| Pipeline | pipeline 位置 | `PipelineStageStarted.stage_index` + `PipelineItemDone.item_index` | 格式 `Stage {n+1}/{total} ({label}), Item {i}`；不在 pipeline 中时省略 |
| Elapsed | 执行耗时 | `AgentDone.elapsed_ms` | 格式化为 `{s}.{ms}s` |

#### Execution 字段

| 字段 | 含义 | 计算方式 |
|------|------|----------|
| Rounds | 对话轮数 | 累计 `ProgressDelta::Message` 出现次数（每次 LLM 回复算一轮） |
| Tool Calls (总数) | 工具调用总次数 | 累计所有 `ProgressDelta::ToolCall` |
| Tool Calls (明细) | 按工具名分组 | `ProgressDelta::ToolCall.name` → 计数；按调用次数降序 |
| File Edits (总数) | 文件编辑次数 | 累计 `ProgressDelta::FileEdit` |
| File Edits (明细) | 被编辑的文件路径 | `ProgressDelta::FileEdit.path` 去重列表 |

#### status 非 ok 时的展示规则

| Status | Output 区块 | Findings 区块 |
|--------|-------------|---------------|
| ok | 正常 JSON 输出 | 正常显示 |
| error | 显示 `"error": "{status}"` | 空或显示已有的 partial findings |
| cancelled | 显示 `"cancelled"` | 空 |
| timed_out | 显示 `"timed_out"` | 空 |

---

### 7.2 Pipeline 聚合 `{run_id}/pipeline_{N}/_summary.md`

#### 完整示例

```markdown
# Pipeline: 2 stages × 3 items

> max_inflight=4 · total elapsed 8.5s

## Stage Overview

| Stage | Label    | Agents |
|-------|----------|--------|
| 0     | analyze  | 3      |
| 1     | assess   | 3      |

## Results Matrix

| Item | Stage 0 (analyze)                          | Stage 1 (assess)                           |
|------|--------------------------------------------|--------------------------------------------|
| 0    | ok · 1,234 tok · 1.2s · [→](../{A0}/report.md) | ok · 567 tok · 0.8s · [→](../{A3}/report.md) |
| 1    | ok · 1,456 tok · 1.5s · [→](../{A1}/report.md) | error · 0 tok · 0.3s · [→](../{A4}/report.md) |
| 2    | ok · 1,100 tok · 1.0s · [→](../{A2}/report.md) | ok · 600 tok · 0.7s · [→](../{A5}/report.md) |

## Totals

| Metric        | Value     |
|---------------|-----------|
| OK            | 5/6       |
| Failed        | 1/6       |
| Total Tokens  | 4,957     |
| Total Elapsed | 8.5s      |
```

#### 区块说明

| 区块 | 内容 | 数据来源 |
|------|------|----------|
| **标题** | `{stages} stages × {items} items` | `PipelineStarted.total_stages` + `items` |
| **描述引用行** | `max_inflight` + 总耗时 | pipeline 运行参数 + `PipelineDone` 时间差 |
| **Stage Overview** | 每个 stage 的 label 和 agent 数 | `PipelineStageStarted.{stage_index, label, agents_in_stage}` |
| **Results Matrix** | item × stage 的结果矩阵 | `PipelineItemDone`（per item per stage）；`[→]` 链接到对应 agentId 的 report.md |
| **Totals** | 汇总统计 | 从矩阵聚合：OK/Failed 计数、token 求和、elapsed 求和 |

#### Results Matrix 单元格格式

```
{status} · {tokens} tok · {elapsed}s · [→](../{agentId}/report.md)
```

| 字段 | 来源 | 备注 |
|------|------|------|
| status | `PipelineItemDone.status` | 文本着色：ok=绿、error=红（纯文本用 emoji 标记） |
| tokens | `PipelineItemDone.tokens.total()` | 修复 bug 后传播实际值 |
| elapsed | `PipelineItemDone.elapsed_ms` | 格式化为秒 |
| `[→]` 链接 | 通过 `agent_id` 时序关联 | ⚠️ `PipelineItemDone` 当前不携带 `agent_id`，需通过同窗口的 `AgentDone` 推断（见 §13 待定项） |

---

### 7.3 Parallel 聚合 `{run_id}/parallel_{span_id}/_summary.md`

#### 完整示例

```markdown
# Parallel: 4 items

> elapsed 15.2s

| # | Status | Tokens | Elapsed | Report |
|---|--------|--------|---------|--------|
| 0 | ok     | 1,234  | 2.1s    | [→](../{A0}/report.md) |
| 1 | ok     | 1,456  | 2.5s    | [→](../{A1}/report.md) |
| 2 | error  |    0   | 0.3s    | [→](../{A2}/report.md) |
| 3 | ok     | 1,100  | 1.8s    | [→](../{A3}/report.md) |

## Totals

| Metric        | Value     |
|---------------|-----------|
| OK            | 3/4       |
| Failed        | 1/4       |
| Total Tokens  | 3,790     |
| Total Elapsed | 15.2s     |
```

#### 区块说明

| 区块 | 内容 | 数据来源 |
|------|------|----------|
| **标题** | `{count} items` | `ParallelStarted.count` |
| **描述引用行** | 总耗时 | `ParallelDone.elapsed_ms` |
| **结果表** | per-item 状态/token/耗时/链接 | `ParallelDone.results`（JSON 数组）；per-item token 从 `AgentDone` 时序关联 |
| **Totals** | 汇总统计 | 从结果表聚合 |

#### 数据来源限制

`ParallelDone.results` 是一个 JSON 数组，每个元素是一个 agent result（含 `ok`/`output`）。但 `ParallelDone` **不携带 per-item agent_id**，因此：
- token 数据需从同时间窗口的 `AgentDone` 事件时序关联推断
- `[→]` 链接可能不准确（并发完成顺序不保证）
- 备用方案：如果关联失败，表格省略链接列，只显示 `ok`/`failed` 计数

---

### 7.4 运行总览 `{run_id}/_summary.md`

#### 完整示例

```markdown
# Run `0192...ef78`

> Architecture review of the luft codebase

## Overview

| Field         | Value                                  |
|---------------|----------------------------------------|
| Status        | completed                              |
| Total Tokens  | 15,234 (in: 10,000 / out: 5,234)       |
| Total Elapsed | 45.2s                                  |
| Agents        | 12 (ok: 10 / error: 1 / timed_out: 1)  |
| Pipelines     | 1 (6 agents, 2 stages)                 |
| Parallels     | 1 (4 agents)                           |

## Phase Tree

```
discover subsystems (1 agent)
├─ review subsystem: core (3 agents)
│   ├─ enumerate modules
│   └─ pipeline (2 stages, 3 items)
├─ review subsystem: runtime (2 agents)
│   ├─ enumerate modules
│   └─ pipeline (2 stages, 2 items)
synthesize architecture review (1 agent)
```

## Agents

| # | Name             | Agent ID    | Phase                | Status | Tokens | Elapsed | Rounds | Tools | Report |
|---|------------------|-------------|----------------------|--------|--------|---------|--------|-------|--------|
| 0 | discover         | 0192...ab12 | discover subsystems  | ok     | 1,234  | 3.1s    | 2      | 4     | [→]    |
| 1 | enumerate        | 0192...cd34 | enumerate modules    | ok     |   890  | 2.0s    | 1      | 3     | [→]    |
| 2 | analyze_auth     | 0192...ef56 | analyze auth         | ok     | 1,801  | 5.2s    | 3      | 7     | [→]    |
| 3 | analyze_db       | 0192...de90 | analyze db           | error  |   234  | 0.3s    | 1      | 1     | [→]    |

## Pipelines

| # | Stages | Items | OK | Failed | Total Tokens | Summary |
|---|--------|-------|----|--------|--------------|---------|
| 0 | 2      | 3     | 5  | 1      | 4,957        | [→]     |

## Parallels

| # | Items | OK | Failed | Total Tokens | Summary |
|---|-------|----|--------|--------------|---------|
| 0 | 4     | 3  | 1      | 3,790        | [→]     |

## Errors

| Agent ID    | Phase         | Status | Detail              |
|-------------|---------------|--------|---------------------|
| 0192...de90 | assess db     | error  | output schema val.. |

## Final Report

```json
{
  "project": "luft",
  "subsystems": [...],
  "synthesis": {...}
}
```
```

#### 区块说明

| 区块 | 内容 | 数据来源 | 展示规则 |
|------|------|----------|----------|
| **标题** | `# Run \`{id_short}\`` | `RunStarted.run_id` | 前 12 位缩写 |
| **描述引用行** | `> {task}` | `RunStarted.task` | 原始任务描述（NL 或 workflow 文件路径）；超 100 字符截断 |
| **Overview** | 全局统计 | `RunDone` + 累计数据 | 表格 |
| **Phase Tree** | phase 嵌套树 | `PhaseSpanStarted/Done` + `PhaseStarted` | ASCII 树，显示每 span 的 agent 数；见下方说明 |
| **Agents** | 全部 agent 列表 | 累计所有 `AgentDone` | 表格，按序号排序；`[→]` 链接到 `{agentId}/report.md` |
| **Pipelines** | 全部 pipeline 列表 | 累计所有 `PipelineDone` | 表格；`[→]` 链接到 `pipeline_{N}/_summary.md` |
| **Parallels** | 全部 parallel 列表 | 累计所有 `ParallelDone` | 表格；`[→]` 链接到 `parallel_{span_id}/_summary.md` |
| **Errors** | 失败 agent 清单 | 筛选 status ≠ ok 的 agent | 表格；仅当有失败时显示；Detail 从 `AgentDone.output` 提取 error 字段 |
| **Final Report** | `report()` 的最终值 | `ReportEmitted.report` 或 `RunDone.report` | JSON code block（同 `write_report` 逻辑） |

#### Overview 字段

| 字段 | 含义 | 计算方式 |
|------|------|----------|
| Status | 运行结束状态 | `RunDone.status`：completed / failed / cancelled / partial |
| Total Tokens | 全局 token 消耗 | `RunDone.total_tokens.total()`；括号内拆分 in/out |
| Total Elapsed | 总耗时 | `RunDone` 时间戳 - `RunStarted` 时间戳 |
| Agents | agent 统计 | 总数 + 按 status 分组计数 |
| Pipelines | pipeline 统计 | 个数 + 总 agent 数 + 平均 stage 数 |
| Parallels | parallel 统计 | 个数 + 总 agent 数 |

#### Agents 表字段

| 列 | 含义 | 来源 |
|----|------|------|
| # | 序号 | `AgentDone.agent_seq` |
| Name | agent 短标签 | `AgentDone.name`（None 时显示 `-`） |
| Agent ID | agent UUID 缩写 | `AgentDone.agent_id` 前 12 位 |
| Phase | 所属 phase 名称 | `PhaseStarted.label` |
| Status | 结束状态 | `AgentDone.status` |
| Tokens | token 总数 | `AgentDone.tokens.total()` |
| Elapsed | 耗时 | `AgentDone.elapsed_ms` 格式化 |
| Rounds | 对话轮数 | 累计 `ProgressDelta::Message` |
| Tools | 工具调用次数 | 累计 `ProgressDelta::ToolCall` |
| Report | 链接到详细报告 | `[→]` → `{seq}_{name}/report.md` |

#### Phase Tree 构建规则

从 `PhaseSpanStarted`（含 `parent_id`、`depth`）和 `PhaseStarted` 事件构建嵌套树：

```
{phase_label} ({agent_count} agents)
├─ {child_span_label} ({agent_count} agents)
│   └─ {inner_phase_label}
└─ {child_span_label} ({agent_count} agents)
```

- 使用 `PhaseSpanStarted.parent_id` 构建父子关系
- `PhaseStarted`（非 span）作为 span 内叶子节点显示
- agent_count：该 span/phase 范围内的 `AgentStarted` 事件计数
- pipeline 在 span 内额外标注 `(N stages, M items)`

#### Errors 表展示规则

- 仅当至少一个 agent status ≠ ok 时显示此区块
- Detail 列从 `AgentDone.output` 尝试提取 `error` / `message` 字段；无结构化错误信息时显示 status 原文
- 按 severity 排序：error → timed_out → cancelled

---

## 8. 文件目录结构

```
{runs_base_dir}/                              ← ~/.luft/runs/ (或配置)
├── arch-review_1781980050/                   ← run_dir_name (slug + timestamp)
│   ├── journal.db                            ← 现有：SQLite journal
│   ├── events.jsonl                          ← 现有：事件日志（可选）
│   └── 01923456-789a-def0-1234-567890abcdef/  ← {run_id} (UUID v7)
│       ├── _summary.md                       ← 运行总览
│       ├── _report.md                        ← report() 最终值
│       ├── 00_discover_subsystems/           ← {seq}_{name}
│       │   └── report.md                     ← 单 agent 报告
│       ├── 01_enumerate_modules/
│       │   └── report.md
│       ├── 02_analyze_auth/
│       │   └── report.md
│       ├── 03/                               ← 无 name 时：{seq}
│       │   └── report.md
│       ├── pipeline_0/
│       │   └── _summary.md                   ← pipeline 聚合
│       └── parallel_42/
│           └── _summary.md                   ← parallel 聚合
```

目录命名规则：`{agent_seq:02}_{name}`，如 `00_discover`、`03_analyze_auth`。name 为 None 时只用 `{agent_seq:02}`。

---

## 9. CLI 接入

### 9.1 新增 CLI flag

```rust
// src/commands/mod.rs 或 RunArgs
pub struct RunArgs {
    // ... 现有字段 ...
    /// Disable writing artifact reports to disk.
    #[arg(long = "no-artifacts")]
    pub no_artifacts: bool,
}
```

### 9.2 run_headless 改动

```rust
async fn run_headless(
    run_ctx: RunContext,
    rt: Runtime,
    script: String,
    output: Option<PathBuf>,
    mut logger: Option<EventLogger>,
    artifacts_dir: Option<PathBuf>,    // 新增：None = 禁用
) -> Result<()> {
    // ...
    let mut artifact_writer = artifacts_dir.map(|dir| ArtifactWriter::new(dir, run_ctx.run_id));

    let printer = tokio::spawn(async move {
        let mut renderer = PhaseRenderer::new(tty);
        let skipped = drain_events(rx, |evt| {
            renderer.handle(evt);
            if let Some(l) = logger.as_mut() {
                let _ = l.write(evt);
            }
            if let Some(w) = artifact_writer.as_mut() {
                w.handle(evt);
            }
        }).await;
        // ...
    });
}
```

### 9.3 调用链

```
run_workflow()
  → runs_base_dir()
  → assign_dir_name(spec, &base_dir)         // 确定 run_dir_name
  → let artifacts_dir = if args.no_artifacts { None }
                        else { Some(base_dir.join(&spec.run_dir_name).join(spec.run_id.to_string())) }
  → run_headless(ctx, rt, script, output, logger, artifacts_dir)
```

---

## 10. 改动文件清单

| # | 文件 | 改动 | 类型 |
|---|------|------|------|
| 1 | `src/core/contract/event.rs` | `AgentDone` 加 `name`/`agent_seq`/`output`/`findings`/`prompt`；`AgentStarted` 加 `name`/`agent_seq` | 事件扩充 |
| 2 | `src/core/contract/backend.rs` | `AgentTask` 加 `name: Option<String>` + `agent_seq: u32` | 契约扩充 |
| 3 | `src/runtime/sdk/mod.rs` | `SdkContext` 加 `agent_seq_counter: Arc<AtomicU32>` | 计数器 |
| 4 | `src/runtime/sdk/task.rs` | `build_task()` 从 Lua opts 读 `name`，`fetch_add` 分配 `agent_seq` | 构造 |
| 5 | `src/core/scheduler/mod.rs` | 发射 `AgentStarted`/`AgentDone` 时带上 `name`/`agent_seq`/`output`/`findings`/`prompt` | 发射点 |
| 6 | `src/runtime/pipeline.rs` | `PipelineItemDone.tokens` 传播实际值 | Bug 修复 |
| 7 | `src/commands/artifact_writer.rs` | 新建模块 | 新增 |
| 8 | `src/commands/mod.rs` | 声明 `pub mod artifact_writer` | 新增 |
| 9 | `src/commands/run.rs` | `RunArgs` 加 `--no-artifacts`；`run_headless` 接入 `ArtifactWriter` | CLI |
| 10 | `src/commands/run.rs` 等 | 所有构造 `AgentStarted`/`AgentDone` 的测试代码补字段 | 测试适配 |

**编译器兜底**：所有构造 `AgentStarted` / `AgentDone` 的地方编译时会强制补上新字段。

---

## 11. 实现阶段

### Phase 1：事件扩充 + Bug 修复（前置依赖）

**Task 1.1** — 扩充 `AgentTask` 契约
- 文件：`src/core/contract/backend.rs`
- 加 `name: Option<String>`、`agent_seq: u32`（均 `#[serde(default)]`）

**Task 1.2** — 扩充 `AgentStarted` + `AgentDone` 事件
- 文件：`src/core/contract/event.rs`
- `AgentStarted` 加 `name` / `agent_seq`
- `AgentDone` 加 `name` / `agent_seq` / `output` / `findings` / `prompt`（均 `#[serde(default)]`）

**Task 1.3** — SdkContext 加 `agent_seq_counter`
- 文件：`src/runtime/sdk/mod.rs`
- 新增 `agent_seq_counter: Arc<AtomicU32>`，与 `phase_counter` 同级

**Task 1.4** — `build_task` 读取 name + 分配 seq
- 文件：`src/runtime/sdk/task.rs`
- 从 Lua opts 读 `name` 字段
- `agent_seq_counter.fetch_add(1, Relaxed)` 分配序号
- 填入 `AgentTask`

**Task 1.5** — 改发射点
- 文件：`src/core/scheduler/mod.rs`
- 发射 `AgentStarted` 时带上 `task.name` / `task.agent_seq`
- 发射 `AgentDone` 时带上 `task.name` / `task.agent_seq` / `result.output` / `result.findings` / `task.prompt`

**Task 1.6** — 修复 PipelineItemDone token bug
- 文件：`src/runtime/pipeline.rs`
- 将 `TokenUsage::default()` 改为实际值

**Task 1.7** — 修复现有测试
- 所有手动构造 `AgentStarted` / `AgentDone` 的测试补上新字段

### Phase 2：ArtifactWriter 核心

**Task 2.1** — 新建 `ArtifactWriter` 结构体
- 文件：`src/commands/artifact_writer.rs`
- 实现 `new()`、`handle(&mut self, evt: &AgentEvent)`

**Task 2.2** — 实现单 agent report 写入
- `write_agent_report()` — 生成 §7.1 格式的 Markdown
- 写入 `{base}/{agentId}/report.md`

**Task 2.3** — 实现 agent 状态累计
- `AgentStarted` → 初始化 `AgentStats`
- `AgentProgress` → 累计 Message/ToolCall/FileEdit
- `PhaseStarted` → 记录 phase label

### Phase 3：Pipeline / Parallel 聚合

**Task 3.1** — Pipeline 上下文跟踪
- `PipelineStarted` → 初始化 `PipelineContext`
- `PipelineStageStarted` → 更新 current_stage
- `PipelineItemDone` → 记录 per-item per-stage 结果
- `PipelineDone` → 写 `_summary.md`

**Task 3.2** — Parallel 聚合
- `ParallelDone` → 写 `_summary.md`（从 `results` JSON 提取状态/token）

### Phase 4：运行总览 + CLI 接入

**Task 4.1** — 运行总览
- `ReportEmitted` → 写 `_report.md`
- `RunDone` → 写 `_summary.md`（含 agent 表）

**Task 4.2** — CLI flag
- `RunArgs` 加 `--no-artifacts`
- `run_headless` 接入 `ArtifactWriter`

**Task 4.3** — 集成测试
- 端到端验证：跑一个含 pipeline 的 workflow，检查输出目录结构

---

## 12. 测试策略

### 12.1 单元测试

| 测试 | 验证点 |
|------|--------|
| `test_agent_report_basic` | 单 agent 完成后生成正确 Markdown |
| `test_agent_report_with_name` | 有 name 的 agent 标题/目录名正确 |
| `test_agent_report_without_name` | 无 name 的 agent 标题/目录名回退到 `#{seq}` |
| `test_agent_seq_monotonic` | agent_seq 全局单调递增，跨 pipeline/parallel |
| `test_agent_report_with_pipeline` | pipeline 内 agent 报告含 stage/item 信息 |
| `test_pipeline_summary` | pipeline 聚合表行数/列数正确 |
| `test_parallel_summary` | parallel 聚合表数据正确 |
| `test_run_summary` | 总览 token 汇总正确 |
| `test_tool_call_counting` | ProgressDelta::ToolCall 正确累计并分组 |
| `test_rounds_counting` | ProgressDelta::Message 正确计为轮数 |

### 12.2 集成测试

用 mock backend 跑一个含 pipeline 的 workflow，验证：
- `{run_id}/` 目录存在
- 每个 agent 有 `{agentId}/report.md`
- pipeline 有 `_summary.md`
- `_summary.md` 和 `_report.md` 存在

---

## 13. 待定 / 后续

- **Parallel agent_id 关联**：`ParallelDone` 不携带 per-item agent_id，聚合表链接可能不准。后续考虑给 `ParallelDone` 加 per-item agent_id 列表，或在 `ParallelStarted` 后跟踪 agent_id 分配
- **大 output 截断**：output 可能很大（几 KB JSON），是否截断到前 N 行？当前决定不截断
- **resume 兼容**：resume 模式下已完成的 agent 不重跑，ArtifactWriter 不会收到它们的 `AgentDone`——需从 journal 补写，或在 resume 时从 SQLite 读取已有 agent 结果补写
- **并发写入安全**：当前 `ArtifactWriter` 在单一 printer task 中运行，无并发问题。若未来改为多 consumer 并行，需加锁
