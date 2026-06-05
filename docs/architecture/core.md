# core 模块架构

> **冻结合约 + 调度器 + 状态持久化。** Maestro 的地基层，定义所有上层模块共享的类型、并发执行逻辑与断点续跑能力。

源码：[`src/core/`](../../src/core/) ｜ 公开 API：[`src/core/mod.rs`](../../src/core/mod.rs)

---

## 1. 职责与边界

`core` 是**唯一没有上游依赖**的模块（除第三方 crate 外不依赖 Maestro 任何其他模块）。它只定义：

- **合约（contract）**——traits、数据类型、事件枚举、缓存键、Schema 校验。一经评审应极少改动，是 `runtime` / `adapters` / `mcp` / `cli` 的共同基底。
- **调度（scheduler）**——把一个 `AgentTask` 安全地跑成 `AgentResult`：并发上限、配额、重试、取消、事件广播、Schema 校验。
- **持久化（state / journal）**——把运行进度落盘成 `checkpoint.json` + `events.jsonl`，并以 cache key 索引支持 `--resume`。
- **测试后端（mock_backend）**——确定性的 `AgentBackend` 实现，供单测与集成测试复用。

```
        ┌─────────────────────────────────────────────┐
        │  runtime · adapters · mcp · planner · cli    │  上层模块
        └───────────────────────┬─────────────────────┘
                                │ 依赖
        ┌───────────────────────▼─────────────────────┐
        │                    core                      │
        │  contract  ◀── scheduler ──▶ journal ──▶ state│
        │     ▲                              mock_backend│
        └─────┴────────────────────────────────────────┘
                （core 内部依赖：scheduler/journal/state 依赖 contract）
```

**关键边界约定**：`AgentBackend` trait 是 `core` 与 `adapters` 的接缝；`AgentEvent` 广播通道是 `core` 与 `cli`/持久化的接缝；`AgentCacheKey` 是 `core` 与 `runtime`（resume）的接缝。

---

## 2. 内部结构

| 文件 | 职责 |
|------|------|
| [`contract/ids.rs`](../../src/core/contract/ids.rs) | `RunId`/`AgentId`（uuid v7）、`PhaseId`（u32）、`TokenUsage`（含 `Add`/`AddAssign`） |
| [`contract/backend.rs`](../../src/core/contract/backend.rs) | `AgentBackend` trait、`AgentTask`、`AgentResult`、`AgentStatus`、`RunContext`、`ToolPolicy`、`McpEndpoint`、`BackendError`、`AgentCapabilities`、`Artifact`、`LogRef` |
| [`contract/event.rs`](../../src/core/contract/event.rs) | `AgentEvent`（带标签枚举，~12 个变体）、`ProgressDelta`、`RunStatus`、`LogLevel`、`EventSender = broadcast::Sender<AgentEvent>` |
| [`contract/finding.rs`](../../src/core/contract/finding.rs) | `Finding`、`Severity`、`Location`——数据面输出契约 |
| [`contract/cache.rs`](../../src/core/contract/cache.rs) | `agent_cache_key()`——§1.5 冻结契约版缓存键（blake3 + NFC 归一化） |
| [`contract/schema.rs`](../../src/core/contract/schema.rs) | `validate_output()`——基于 `jsonschema` Draft7 的结构化输出校验 |
| [`scheduler/mod.rs`](../../src/core/scheduler/mod.rs) | `Scheduler`、`run_agent`/`run_parallel`/`cancel_*`、`JournalCallback` trait |
| [`scheduler/config.rs`](../../src/core/scheduler/config.rs) | `SchedulerConfig`、`RetryPolicy`（指数退避） |
| [`scheduler/registry.rs`](../../src/core/scheduler/registry.rs) | `BackendRegistry`——`id → Arc<dyn AgentBackend>` |
| [`scheduler/error.rs`](../../src/core/scheduler/error.rs) | `SchedulerError` |
| [`journal.rs`](../../src/core/journal.rs) | `JournalStore`（cache key 索引）、`AgentCacheKey`、`ResumeContext`、`RunCreationMode`、`gc_runs()` |
| [`state.rs`](../../src/core/state.rs) | `RunStore`、`RunCheckpoint`、`AgentResultCache`、`CheckpointStatus`、`PhaseSummary`、全局 `RUN_STORES` |
| [`mock_backend.rs`](../../src/core/mock_backend.rs) | `MockBackend`、`MockBehavior`、`FailKind`——确定性测试后端 |

---

## 3. 核心抽象

### 3.1 AgentBackend trait

所有 LLM 后端的统一接口——**prompt 进，结构化 `AgentResult` 出**。

```rust
#[async_trait]
pub trait AgentBackend: Send + Sync {
    fn id(&self) -> &'static str;                 // 稳定后端 id，如 "opencode"
    fn capabilities(&self) -> AgentCapabilities;  // 能力声明（v0.1 仅记录，路由在 v0.2）
    async fn run(&self, task: AgentTask, ctx: RunContext)
        -> Result<AgentResult, BackendError>;
}
```

- `AgentTask`：`prompt`（必填）、`model`、`allowlist: ToolPolicy`、`workdir`、`mcp_endpoint`、`timeout`、`output_schema`。
- `AgentResult`：`status`、`output: serde_json::Value`、`findings`、`tokens_used`、`artifacts`、`logs`。
- `RunContext`：`run_id` + `cancel: CancellationToken` + `events: EventSender`。实现方**应观察 `ctx.cancel`**，触发时尽快返回 `BackendError::Cancelled`。
- `BackendError::is_retryable()`：仅 `Timeout` 与 `Spawn(_)` 可重试，其余（`Protocol`/`Parse`/…）不可重试。

### 3.2 事件总线（AgentEvent）

`AgentEvent` 是**唯一的可观测性数据源**：headless 把每条事件序列化为一行 JSONL，TUI 把同一条流投影成 phase→agent 视图，state 把它持久化。变体覆盖运行生命周期：`RunStarted` / `PhaseStarted` / `AgentStarted` / `AgentProgress` / `AgentDone` / `PhaseDone` / `RunDone` / `Log`，以及 pipeline 专用的 `PipelineStarted` / `PipelineStageStarted` / `PipelineItemDone` / `PipelineDone`。

传输用 `tokio::sync::broadcast`——多消费者（持久化任务、TUI、headless drain）各自 `subscribe()`/`resubscribe()`，互不阻塞。

### 3.3 缓存键（两套实现，注意区分）

| 实现 | 输入 | 归一化 | 实际使用方 |
|------|------|--------|-----------|
| `contract::cache::agent_cache_key()` | backend_id + model + prompt + phase | NFC + 折叠空白 | §1.5 冻结契约，当前 runtime 未直接调用 |
| `journal::AgentCacheKey::new()` | prompt + model + phase | 折叠空白（无 NFC） | **runtime resume 路径实际使用** |

两者都用 blake3 + `\0` 分隔符防字段拼接碰撞。**`runtime` 的 `agent()`/`parallel()` 用的是 `journal::AgentCacheKey`**——这是 resume 命中的真正依据。两套实现的存在是历史演进残留，统一到单一实现是后续清理项。

---

## 4. Scheduler：从 task 到 result 的安全路径

`Scheduler` 以 `Arc<Scheduler>` 形式被编排协程共享，内部状态：

```rust
struct Scheduler {
    config: SchedulerConfig,
    semaphore: Arc<Semaphore>,              // 全局并发闸
    registry: BackendRegistry,
    runs: DashMap<RunId, RunState>,         // per-run 状态
    journal_callback: Option<Arc<dyn JournalCallback>>,
}
struct RunState {
    quota_used: Arc<AtomicU32>,             // per-run 配额计数
    run_cancel: CancellationToken,          // run 级取消（父）
    events: EventSender,
    agent_cancels: DashMap<AgentId, CancellationToken>,  // agent 级取消（子）
}
```

### `run_agent` 执行流水线

```
backend 查找(registry) → 快照 per-run 句柄(不跨 await 持锁)
  → 配额检查(AtomicU32 fetch_add，超限 → QuotaExceeded)
  → 建立 agent 取消令牌(run_cancel 的子令牌)
  → 获取信号量 permit(等待期间可被取消)
  → emit AgentStarted
  → ┌── 重试循环 ──────────────────────────────┐
    │  backend.run() 包 tokio::timeout(可选)    │
    │  Ok  → 若有 output_schema 则校验 → break  │
    │  Err → 取消? → 不可重试? → 超过 max? →    │
    │        退避 sleep(可被取消) → 重试         │
    └──────────────────────────────────────────┘
  → emit AgentDone(status, tokens, elapsed)
  → journal_callback.on_agent_done()(若配置)
  → drop permit + 清理 agent 令牌
```

| 能力 | 实现 |
|------|------|
| 全局并发上限 | `Semaphore`，默认 `2×CPU` clamp 到 `[4,16]` |
| Per-run 配额 | `AtomicU32`，默认 1000，防 fan-out 失控 |
| Per-agent / per-run 取消 | `CancellationToken` 树：agent 令牌是 run 令牌的子令牌，任一触发都生效 |
| 重试 | 指数退避：默认 `max_attempts=2`、`initial=500ms`、`×2`、`cap=10s`；退避 sleep 可被取消打断 |
| Schema 校验 | `output_schema` 存在时对 `AgentResult.output` 跑 `validate_output()` |
| 事件广播 | 所有状态变化 emit 到 per-run 的 `EventSender` |

**设计注记（§9.2 C1）**：`run_agent` 返回 `Result<AgentResult, SchedulerError>` 而非设计稿里的 `(result, TaskHandle)` 元组——取消通过 `cancel_agent(run_id, agent_id)` 按 id 寻址，因此不需要句柄。

`run_parallel` 用 `futures::join_all` 并发跑一批，**不短路失败**、**结果保持输入顺序**——这是 `parallel()` 原语的栅栏语义基础。

---

## 5. 持久化：state + journal 两层

### 5.1 RunStore（state.rs）——落盘引擎

每个 run 一个目录，两个文件：

| 文件 | 写入方式 | 内容 |
|------|---------|------|
| `events.jsonl` | append + `flush()` | 每事件一行，仅追加 |
| `checkpoint.json` | `fs::write` **全量重写** | 当前快照：状态、phase、`agent_results`、token 累计 |

`append_event()` 同时做两件事：追加事件行，并 `update_from_event()` 把派生状态写回 checkpoint——`AgentDone` 累加 `total_tokens` 并更新 `agent_results`，`PhaseDone` 推进 `current_phase`，`RunDone` 落定终态。

> ⚠️ **准确说明**：checkpoint 当前是 `fs::write` 全量重写，**不是 temp 文件 + rename 的原子交换**；崩溃恰好发生在重写中途存在损坏风险。`events.jsonl` 是追加 + flush，相对更安全。原子化是后续加固项。

全局 `RUN_STORES`（`OnceLock<DashMap<RunId, Arc<RunStore>>>`）让同一进程内多处共享同一 run 的 store，避免 split-brain。

### 5.2 JournalStore（journal.rs）——续跑语义层

`JournalStore` 在 `RunStore` 之上叠加一个 **cache key 索引**（`RwLock<HashMap<String, AgentResultCache>>`），把"已完成 agent 的 O(1) 查询"与"落盘"解耦：

- `open(run_id)`：从 checkpoint 重建索引（同时按 `agent_id` 和 `cache_key_hash` 双重索引），并**拒绝恢复已 Completed/Cancelled 的 run**。
- `record_result()`：runtime 在 agent 完成后调用——**只 upsert checkpoint + 刷新索引，不追加 `AgentDone` 事件**，从而不会与事件驱动的 token 累计重复计数。
- `cache_agent()`：完整持久化 + 追加事件 + 广播（另一条更"重"的路径）。
- `has_completed()` / `get_cached()`：runtime 的 `agent()` 在提交调度前查询，命中则直接复用、跳过执行。
- 作为 `JournalCallback` 实现：scheduler 回调路径按 `agent_id` 落盘（无 cache_key、无 findings）。

`RunCreationMode`（`New`/`Resume`/`Auto`）封装"新建 or 续跑"的解析；`gc_runs()` 清理超期的终态 run。

### 5.3 续跑数据流

```
首跑:  init_run → [agent 完成 → record_result(写索引+checkpoint)]* → RunDone
        磁盘: checkpoint.json(agent_results 带 cache_key_hash) + events.jsonl + workflow.lua

续跑:  open(run_id) → 重建 cache_index
        → 重放同一脚本 → agent() 查 has_completed() → 命中则跳过、未命中才执行
```

---

## 6. 并发与线程模型

- `Scheduler` / `JournalStore` / `RunStore` 全部**内部可变 + `&self`**：`Semaphore` / `AtomicU32` / `DashMap` / `RwLock`，可被多个编排协程并发共享。
- 取消是**令牌树**：run 令牌 → agent 子令牌。父取消则所有子取消；agent 单独取消不影响兄弟。
- 跨 `await` **不持有 `DashMap` 守卫**——`run_agent` 先快照句柄再 await，避免死锁。
- `RunState.events` 既给 scheduler（`AgentStarted`/`AgentDone`）也给 backend（经 `RunContext`），通过 `init_run_with` 注入同一总线，保证事件不分裂。

---

## 7. 设计决策与权衡

- **contract 无上游依赖、改动极慎**：作为"冻结合约"，任何字段变更都会波及全模块；新增能力优先用 `Option`/`#[serde(default)]` 向后兼容（如 `AgentResultCache.cache_key_hash`）。
- **调度集中、后端可插拔**：并发/配额/重试/取消的策略统一在 scheduler，后端只管"一次 prompt→result"，复杂度不外溢。
- **事件总线作为单一事实源**：持久化、TUI、headless 都消费同一条 `AgentEvent` 流，避免多套状态。
- **journal 与 state 分层**：state 负责"怎么落盘"，journal 负责"续跑语义"，cache key 索引只活在内存、由 checkpoint 重建。

---

## 8. 当前状态与局限（v0.1）

- checkpoint 写入**非原子**（见 §5.1）。
- 存在**两套缓存键实现**（见 §3.3），尚未统一。
- `AgentCapabilities` 仅被记录/校验，**能力路由（按 model/能力选后端）留待 v0.2**——当前 `default_backend()` 取注册表第一个。
- `JournalCallback` 回调路径落盘的 `AgentResultCache` **不含 findings 与 cache_key_hash**；runtime 的 `record_result` 路径才是完整的。

---

## 9. 相关文档

- 总览：[../architecture.md](../architecture.md)
- 上层消费者：[runtime.md](./runtime.md)（SDK 如何调用 scheduler/journal）、[adapters.md](./adapters.md)（如何实现 `AgentBackend`）、[cli.md](./cli.md)（如何编排 run 生命周期）
- 旧版设计稿（实现动机）：[../archive/contracts.md](../archive/contracts.md)、[../archive/scheduler.md](../archive/scheduler.md)、[../archive/state.md](../archive/state.md)
