# core 模块架构

> **状态**: 待完善 — 骨架文档，需补充详细内容。

> 冻结合约 + 调度器 + 状态持久化 — 无上游依赖的地基层。

源码：[`src/core/`](../../src/core/) ｜ 公开 API：[`src/core/mod.rs`](../../src/core/mod.rs)

---

## 1. 职责与边界

`core` 是 Luft 的**地基**。它定义了其他所有模块依赖的共享合约（trait、事件、ID），实现了并发调度逻辑，并负责运行状态的持久化与恢复。`core` 不依赖 `runtime`/`adapters`/`planner`/`mcp`/`cli` 中的任何一个。

```
                  cli ──► planner ──► runtime ──► core ◄── adapters
                   └────────────────────────────► core ◄── mcp
```

### 1.1 子模块总览

| 子模块 | 路径 | 职责 |
|--------|------|------|
| `contract` | `contract/` | 冻结合约 — 共享类型、trait、事件枚举、缓存键 |
| `scheduler` | `scheduler/` | 并发调度 — 信号量、配额、重试、取消、事件广播 |
| `journal` | `journal.rs` | JournalStore — cache key 索引、O(1) 查询、ResumeContext |
| `state` | `state.rs` | RunStore / RunCheckpoint — JSONL 事件日志 + checkpoint 落盘 |
| `mock_backend` | `mock_backend.rs` | MockBackend — 确定性测试后端 |

---

## 2. contract — 冻结合约

`contract` 是 `core` 的**合约层**（frozen contracts）。所有模块依赖的类型都定义在此，一旦冻结应极少修改。

```
contract/
├── mod.rs          # 公开 re-export
├── backend.rs      # AgentBackend trait · AgentTask · AgentResult · BackendError
├── event.rs        # AgentEvent 枚举 · EventSender 广播总线
├── cache.rs        # agent_cache_key() 确定性哈希
├── finding.rs      # Finding 结构化输出
├── ids.rs          # RunId · AgentId · PhaseId · TokenUsage
└── schema.rs       # JSON Schema 校验 (validate_output)
```

### 2.1 AgentBackend Trait — `core` ↔ `adapters` 接缝

所有 LLM 后端的统一接口。Prompt 进，结构化 `AgentResult` 出。

```rust
#[async_trait]
pub trait AgentBackend: Send + Sync {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> AgentCapabilities;
    async fn run(&self, task: AgentTask, ctx: RunContext) -> Result<AgentResult, BackendError>;
}
```

- **实现方**: `MockBackend`（`core/mock_backend.rs`，测试用）、`AcpAdapter`（`adapters/`，opencode ACP 后端）
- **RunContext**: 携带 `run_id`、`CancellationToken`、`EventSender`，后端应观察 `ctx.cancel` 并在触发时返回 `BackendError::Cancelled`
- **BackendError**: 区分可重试（`Timeout` / `Spawn`）与非可重试错误；`is_retryable()` 方法供 scheduler 决策

> 待补充: 详细字段说明、能力声明机制、McpEndpoint 注入方式。

### 2.2 AgentEvent 广播总线 — 唯一可观测性数据源

`tokio::sync::broadcast<AgentEvent>` 是整个运行时**唯一的可观测性数据源**。同一条流被持久化（`RunStore.append_event`）、headless 输出（JSONL）同时消费。

```rust
pub type EventSender = tokio::sync::broadcast::Sender<AgentEvent>;
```

事件枚举（21 个变体）覆盖完整生命周期：

| 类别 | 事件 |
|------|------|
| Run | `RunStarted` · `RunDone` |
| Phase | `PhaseStarted` · `PhaseDone` |
| Agent | `AgentStarted` · `AgentProgress` · `AgentDone` |
| SDK 原语 | `ReportEmitted` · `BudgetSet` · `ParallelStarted/Done` · `WorkflowStarted/Done` · `ConvergeStarted/Done` |
| Pipeline | `PipelineStarted` · `PipelineStageStarted` · `PipelineItemDone` · `PipelineDone` |
| 基础设施 | `Log` · `AcpRaw` |

> 待补充: 事件大小限制、背压处理、消费者准入策略。

### 2.3 AgentCacheKey — `core` ↔ `runtime`（resume）接缝

**确定性去重键**，用于 `--resume` 时跳过已完成的 agent 调用。

- **函数** `agent_cache_key(backend_id, model, prompt, phase) → blake3 hex`:
  - 使用 `\0` 分隔符防止字段拼接冲突
  - prompt 经过 NFC 归一化 + 空白折叠 + 换行统一
- **结构体** `AgentCacheKey`（定义于 `journal.rs`）:
  - `hash`: blake3 hex digest
  - `prompt_preview`: 前 80 字符（人类可读）
  - `model` / `phase_id`: 辅助字段

> 待补充: 冲突概率分析、缓存粒度选择依据。

---

## 3. scheduler — 并发调度器

`scheduler` 是 agent 调度的**中央控制器**，统一处理并发限制、配额、重试、取消和事件报告。

```
scheduler/
├── mod.rs          # Scheduler 结构体 + JournalCallback trait
├── config.rs       # SchedulerConfig · RetryPolicy
├── error.rs        # SchedulerError 枚举
└── registry.rs     # BackendRegistry — 后端注册中心
```

### 3.1 Scheduler 核心流程

`run_agent` 的完整路径：

1. **Backend 解析** — 根据 id 查找 `BackendRegistry`，或使用默认后端
2. **配额检查** — 检查 `quota_per_run` 上限
3. **并发控制** — 等待 `Semaphore`（可取消等待）
4. **取消注册** — 创建 `CancellationToken`（run 级 + agent 级）
5. **重试循环** — 对 `Timeout`/`Spawn` 做指数退避重试
6. **Schema 校验重试** — `output_schema` 不匹配时注入错误信息重试
7. **事件报告** — 广播 `AgentStarted` → `AgentDone`
8. **Journal 回调** — 调用 `JournalCallback::on_agent_done` 透明落盘

### 3.2 JournalCallback

```rust
#[async_trait]
pub trait JournalCallback: Send + Sync {
    async fn on_agent_done(
        &self,
        agent_id: AgentId,
        phase_id: PhaseId,
        status: AgentStatus,
        output: serde_json::Value,
        tokens: TokenUsage,
    );
}
```

Scheduler 持有一个可选的 `JournalCallback`（装配为 `JournalStore`），在每个 agent 完成后自动调用。`JournalStore` 同时实现 `on_agent_done` 并通过 `CompositeJournalCallback` 支持链式组合。

### 3.3 配置

```rust
pub struct SchedulerConfig {
    pub max_concurrency: usize,   // 默认 2×cpu，钳制 [4, 16]
    pub quota_per_run: u32,       // 默认 1000
    pub retry: RetryPolicy,
}

pub struct RetryPolicy {
    pub max_attempts: u32,          // 默认 2
    pub initial_backoff: Duration,  // 默认 500ms
    pub backoff_multiplier: f64,    // 默认 2.0
    pub max_backoff: Duration,      // 默认 10s
    pub schema_retry_max: u32,      // 默认 1
}
```

> 待补充: 详细错误处理策略、`run_parallel` 实现、配额回收时机。

---

## 4. state — 状态持久化

状态持久化由两层组成：底层 `RunStore` 负责磁盘 I/O，上层 `JournalStore` 增加 cache key 索引。

### 4.1 RunStore

```
run_dir/
├── checkpoint.json    ← RunCheckpoint（全量状态快照）
└── events.jsonl       ← JSONL 事件日志（追加写）
```

```rust
pub struct RunCheckpoint {
    pub run_id: RunId,
    pub task: String,
    pub status: CheckpointStatus,   // Running | Completed | Failed | Cancelled
    pub current_phase: u32,
    pub completed_phases: Vec<PhaseSummary>,
    pub agent_results: HashMap<AgentId, AgentResultCache>,
    pub findings: Vec<Finding>,
    pub total_tokens: u64,
    pub created_at: u64,
    pub updated_at: u64,
}
```

**关键方法**:
- `init_run(run_id, task)` — 创建 run 目录 + 初始化 checkpoint
- `append_event(event)` — 追加写到 `events.jsonl`
- `save_checkpoint(checkpoint)` — 全量重写 `checkpoint.json`（当前非原子写入）
- `open_run(run_id)` — 从磁盘恢复 `RunCheckpoint`
- `get_event_log()` — 回放全部事件

### 4.2 JournalStore

`JournalStore` 包装 `RunStore`，在其上增加了 cache key 索引：

```
JournalStore
├── inner: Arc<RunStore>
├── cache_index: RwLock<HashMap<String, AgentResultCache>>   // hash → cache
└── event_tx: Option<EventSender>
```

**关键方法**:
- `cache_agent(key, ...)` — 写入 `RunStore.upsert_agent_result` + 更新内存索引
- `record_result(...)` — `cache_agent` 别名，同时广播事件
- `get_cached(key)` — O(1) 查询 cache 索引
- `has_completed(key)` — 判断是否可跳过
- `new(run_dir)` / `open(run_id)` — 初始化或恢复已有 run

### 4.3 恢复流程（Resume）

```rust
pub struct ResumeContext {
    pub run_id: RunId,
    pub checkpoint: RunCheckpoint,
    pub journal: Arc<JournalStore>,
    pub scheduler_config: SchedulerConfig,
    pub backend_registry: BackendRegistry,
}
```

运行时通过 `RunCreationMode` 决定新建还是恢复：检查 `checkpoint.json` 是否存在且状态为 `Running`/不完整，若是则从存档点接续执行，否则从头开始。

> 待补充: 原子写入（fs::write vs temp+rename）、事件回放弃重逻辑、垃圾回收策略。

---

## 5. MockBackend — 确定性测试后端

```rust
pub enum MockBehavior {
    Success { output: Value, tokens: TokenUsage, delay: Duration },
    Fail { kind: FailKind, delay: Duration },
    Hang,
}

pub struct MockBackend {
    id: &'static str,
    behaviors: Vec<MockBehavior>,
    calls: AtomicU32,
}
```

按顺序消费 `behaviors` 列表，支持注入成功/失败/挂起行为，用于 scheduler 和 runtime 的确定性单元测试。

---

## 6. 已冻结内容

以下类型/接口在 `contract/` 中定义，应视为**已冻结**（修改需跨模块协调）：

| 类型 | 文件 | 冻结范围 |
|------|------|---------|
| `AgentBackend` trait | `contract/backend.rs` | 签名与语义 |
| `AgentEvent` 枚举 | `contract/event.rs` | 变体集合与字段 |
| `agent_cache_key()` | `contract/cache.rs` | 算法输出（hash 值） |
| `Finding` | `contract/finding.rs` | 字段结构 |
| `TokenUsage` | `contract/ids.rs` | 字段与 `Add` 语义 |
| `EventSender` | `contract/event.rs` | 类型别名（`broadcast::Sender`） |

---

## 7. 已知现状与局限

- **checkpoint 非原子写入** — `RunStore.save_checkpoint` 使用 `fs::write` 全量重写，非 temp+rename，崩溃时可能产生部分写入
- **Semaphore 动态调整** — `max_concurrency` 在构建后不可变；运行中无法调整
- **cache key 无版本号** — 算法变更会导致已有缓存失效，当前无迁移机制
- **事件通道无背压** — `broadcast::Sender` 的 `lagged` 行为需消费端自行处理
- **gc_runs 仅基于修改时间** — 未考虑仍活跃的 run

> 参见: `architecture.md` §"已知现状与局限" 中的全局要点。
