# Library 化：Workspace 拆分 + Builder API — 实现设计

> **状态**: 📝 设计阶段（2025-08-19）
> **目标**: 将 luft 从单 crate 拆分为 workspace 多 crate 结构，并提供面向 SDK 消费者的 Builder API

---

## 1. 现状

当前 luft 是单 crate（`[lib]` + `[[bin]]`），所有模块暴露在同一个 `lib.rs` 下：

```
src/
├── lib.rs            # pub mod core/adapters/mcp/mock_gen/planner/runtime/service/storage
├── main.rs           # CLI binary
├── core/             # 合约 + 调度 + journal + state
├── runtime/          # Lua VM + SDK primitives
├── adapters/         # ACP backend
├── planner.rs        # NL → Lua
├── service/          # run/query API
├── storage/          # SQLite 持久化
├── mcp.rs            # MCP server
├── mock_gen.rs       # Mock 生成
├── backend.rs        # CLI: backend factory
└── config.rs         # CLI: 配置管理
```

**问题**:

| # | 问题 | 影响 |
|---|------|------|
| 1 | 下游想实现自定义 backend 必须拉 `mlua`、`sqlx`、`agent-client-protocol` 全部重依赖 | 依赖膨胀 |
| 2 | 没有结构化的 Builder API，外部用户需要理解 `RunSpec` / `prepare` / `execute` / `JournalStore` 内部概念 | 使用门槛高 |
| 3 | 全用 `anyhow::Result`，下游无法 `match` 具体错误类型 | 错误处理困难 |
| 4 | CLI 依赖（`clap`、`indicatif`、`console`）与 library 依赖混在一起 | 边界模糊 |

---

## 2. 目标架构

### 2.1 Workspace 结构

```
luft/                         # workspace root
├── Cargo.toml                   # [workspace] members + resolver
├── crates/
│   ├── luft-core/            # 合约 + 调度 + journal + state + mock_gen
│   ├── luft-storage/         # SQLite 持久化（隔离 sqlx）
│   ├── luft-runtime/         # Lua VM + SDK（隔离 mlua）
│   ├── luft-adapters/        # ACP backend（隔离 agent-client-protocol）
│   ├── luft-planner/         # NL → Lua
│   ├── luft-service/         # 组合层：run/query API（独立 crate）
│   └── luft/                 # 聚合：re-export + Builder + LuftError + mcp
├── src/                         # luft-cli binary
│   ├── main.rs
│   ├── commands/
│   ├── backend.rs
│   └── config.rs
├── examples/
└── workflows/
```

### 2.2 Crate 依赖图（无环）

```
luft-core       →  (无内部依赖)
luft-storage    →  luft-core
luft-runtime    →  luft-core
luft-adapters   →  luft-core
luft-planner    →  luft-core, luft-runtime
luft-service    →  luft-core, luft-runtime, luft-storage, luft-planner
luft            →  以上全部 + Builder API + LuftError + mcp
luft-cli        →  luft
```

### 2.3 各 Crate 职责

#### `luft-core` — 合约层

下游实现自定义 backend 时唯一需要的轻量 crate。

| 模块 | 内容 | 外部依赖 |
|------|------|----------|
| `contract/backend.rs` | `AgentBackend` trait, `AgentTask`, `AgentResult`, `RunContext`, `BackendError` | async-trait, serde, tokio-util |
| `contract/event.rs` | `AgentEvent`, `RunStatus`, `EventSender` | serde, tokio (broadcast) |
| `contract/ids.rs` | `RunId`, `AgentId`, `PhaseId`, `TokenUsage` | uuid, serde |
| `contract/finding.rs` | `Finding` | serde |
| `contract/schema.rs` | schema 验证类型 | serde_json |
| `contract/cache.rs` | `agent_cache_key` | blake3 |
| `scheduler/` | `Scheduler`, `SchedulerConfig`, `BackendRegistry`, `RetryPolicy` | dashmap, jsonschema, tokio |
| `journal.rs` | `JournalStore` | blake3, serde_json |
| `state.rs` | `RunCheckpoint`, `RunStore`, `CheckpointStatus` | serde_json, chrono |
| `run_dir.rs` | `compose`, `derive_slug`, `ensure_unique` | — |
| `mock_backend.rs` | `MockBackend`, `MockBehavior`（`#[cfg(feature="testing")]`） | — |
| `mock_file_backend.rs` | `MockFileBackend`（`#[cfg(feature="testing")]`） | — |

```toml
[features]
testing = []   # 导出 MockBackend / MockFileBackend
```

**不含：** mlua, sqlx, agent-client-protocol, clap, indicatif

#### `luft-storage` — SQLite 持久化

当前 `src/storage/` 搬入。把 sqlx 隔离在这里。

```toml
[dependencies]
luft-core = { path = "../luft-core" }
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "sqlite", "chrono", "uuid", "json", "macros", "migrate"] }
```

#### `luft-runtime` — Lua 编排引擎

当前 `src/runtime.rs` + `src/runtime/` 搬入。把 mlua 隔离在这里。

```toml
[dependencies]
luft-core = { path = "../luft-core" }
mlua = { version = "0.10", features = ["lua54", "vendored", "async", "serialize", "send"] }
```

#### `luft-adapters` — ACP Backend

当前 `src/adapters/` 搬入。把 agent-client-protocol 隔离在这里。

```toml
[dependencies]
luft-core = { path = "../luft-core" }
agent-client-protocol = { version = "0.14", features = ["unstable_end_turn_token_usage"] }
```

#### `luft-planner` — NL → Lua

当前 `src/planner.rs` 搬入。依赖 runtime 的 `validate_script`。

#### `luft-service` — 组合层

当前 `src/service/` 搬入。独立 crate，组合 core/runtime/storage/planner 的编排逻辑。

内部继续使用 `anyhow::Result`，不定义独立 error 类型。

```toml
[dependencies]
luft-core    = { path = "../luft-core" }
luft-runtime = { path = "../luft-runtime" }
luft-storage = { path = "../luft-storage" }
luft-planner = { path = "../luft-planner" }
```

#### `luft` — 聚合 crate

re-export 全部子 crate + Builder API + `LuftError` + `mcp` 模块。

```toml
[dependencies]
luft-core     = { path = "../luft-core" }
luft-storage  = { path = "../luft-storage" }
luft-runtime  = { path = "../luft-runtime" }
luft-adapters = { path = "../luft-adapters" }
luft-planner  = { path = "../luft-planner" }
luft-service  = { path = "../luft-service" }

[features]
testing = ["luft-core/testing"]
unstable_end_turn_token_usage = ["luft-adapters/unstable_end_turn_token_usage"]
```

`src/lib.rs`:
```rust
pub use luft_core;
pub use luft_storage;
pub use luft_runtime;
pub use luft_adapters;
pub use luft_planner;
pub use luft_service;

mod mcp;
mod builder;
mod error;
pub mod prelude;

pub use builder::{Luft, LuftBuilder, RunHandle, RunOutcome};
pub use error::LuftError;
```

---

## 3. Builder API 设计

### 3.1 LuftBuilder

```rust
pub struct LuftBuilder {
    backend: Option<Arc<dyn AgentBackend>>,
    base_dir: PathBuf,
    concurrency: usize,
    planner_config: PlannerConfig,
    exec_limits: ExecLimits,
}

impl LuftBuilder {
    pub fn new() -> Self;

    /// 注册 agent backend（必须设置，否则 build() 返回 BackendNotConfigured）。
    pub fn backend<B: AgentBackend + 'static>(self, b: B) -> Self;

    /// Run 数据目录，默认 `.luft/runs`。
    pub fn base_dir<P: Into<PathBuf>>(self, dir: P) -> Self;

    /// 最大并发 agent 数，默认 4。
    pub fn concurrency(self, n: usize) -> Self;

    /// Planner 配置（NL → Lua 的 prompt 模板等）。
    pub fn planner_config(self, cfg: PlannerConfig) -> Self;

    /// Lua VM 执行限制（超时、指令数等）。
    pub fn exec_limits(self, limits: ExecLimits) -> Self;

    pub fn build(self) -> Result<Luft, LuftError>;
}

impl Default for LuftBuilder {
    fn default() -> Self { Self::new() }
}
```

### 3.2 Luft

```rust
pub struct Luft {
    backend: Arc<dyn AgentBackend>,
    base_dir: PathBuf,
    concurrency: Option<usize>,
    planner_config: PlannerConfig,
    exec_limits: ExecLimits,
}

impl Luft {
    pub fn builder() -> LuftBuilder;

    // ── 异步执行：返回 RunHandle ──

    pub async fn start_script(&self, lua: &str) -> Result<RunHandle, LuftError>;
    pub async fn start_workflow(&self, path: &Path) -> Result<RunHandle, LuftError>;
    pub async fn start_nl(&self, nl: &str) -> Result<RunHandle, LuftError>;
    pub async fn start_resume(&self, run_dir: &str) -> Result<RunHandle, LuftError>;

    // ── 便捷执行：start + join 一步到位 ──

    pub async fn run_script(&self, lua: &str) -> Result<RunOutcome, LuftError>;
    pub async fn run_workflow(&self, path: &Path) -> Result<RunOutcome, LuftError>;
    pub async fn run_nl(&self, nl: &str) -> Result<RunOutcome, LuftError>;
    pub async fn run_resume(&self, run_dir: &str) -> Result<RunOutcome, LuftError>;

    // ── 查询（同步，读 checkpoint/events） ──

    pub fn status(&self, run_dir: &str) -> Result<Option<StatusOutput>, LuftError>;
    pub fn list(&self) -> Result<Vec<StatusOutput>, LuftError>;
    pub fn events(&self, run_dir: &str) -> Result<Vec<AgentEvent>, LuftError>;
    pub fn report(&self, run_dir: &str) -> Result<ReportStatus, LuftError>;
    pub fn findings(&self, run_dir: &str) -> Result<Vec<Finding>, LuftError>;
    pub fn cancel(&self, run_dir: &str) -> Result<(), LuftError>;
}
```

### 3.3 RunHandle

```rust
pub struct RunHandle {
    run_id: RunId,
    run_dir_name: String,
    join: tokio::task::JoinHandle<Result<Result<Value, ScriptError>>>,
    cancel: CancellationToken,
    events: broadcast::Sender<AgentEvent>,
}

impl RunHandle {
    pub fn run_id(&self) -> RunId;
    pub fn run_dir_name(&self) -> &str;

    /// 订阅事件流。每次调用返回一个新的 Receiver。
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent>;

    /// 触发取消（fire-and-forget）。
    pub fn cancel(&self);

    /// 等待运行完成，消费 handle。
    pub async fn join(self) -> Result<RunOutcome, LuftError>;
}

/// 支持 `let outcome = handle.await?;`
impl IntoFuture for RunHandle {
    type IntoFuture = JoinFuture;
    type Output = Result<RunOutcome, LuftError>;
}
```

### 3.4 RunOutcome

```rust
pub struct RunOutcome {
    pub run_id: RunId,
    pub run_dir_name: String,
    pub result: Result<serde_json::Value, ScriptError>,
}
```

### 3.5 start 内部流程

```
start_script(lua)
  │
  ├─ ScriptSource::Script(lua) → resolve_fresh() → RunSpec
  │
  ├─ assign_dir_name(spec, base_dir) + create_dir_all()
  │
  ├─ broadcast::channel(256) → (tx, _)
  ├─ CancellationToken::new()
  ├─ RunContext { run_id, cancel, events: tx }
  │
  ├─ prepare(spec, backend, base_dir, run_ctx, concurrency)
  │   └─ 内部 spawn journal forwarder（监听 events → journal + sqlite）
  │
  ├─ tokio::spawn(execute(run_ctx, runtime, script))
  │
  └─ RunHandle { run_id, run_dir_name, join, cancel, events: tx }
```

`join()` 消费 handle → handle drop → broadcast sender drop → journal forwarder 自动终止。

---

## 4. 错误类型

```rust
#[derive(thiserror::Error, Debug)]
pub enum LuftError {
    #[error(transparent)]
    Backend(#[from] BackendError),

    #[error(transparent)]
    Script(#[from] ScriptError),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Scheduler(#[from] SchedulerError),

    #[error("run not found: {0}")]
    RunNotFound(String),

    #[error("run not resumable: {0}")]
    NotResumable(String),

    #[error("backend not configured")]
    BackendNotConfigured,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
```

`Other` 作为 catch-all，后续逐步收窄为具体 variant。

`luft-service` 内部继续用 `anyhow::Result`，聚合层 `Luft` 的方法负责将 `anyhow::Error` 转换为 `LuftError`（通过 `?` + `From`，或显式 `.map_err()`）。

---

## 5. Prelude 模块

```rust
// crates/luft/src/prelude.rs

pub use luft_core::contract::backend::{
    AgentBackend, AgentTask, AgentResult, AgentCapabilities, AgentStatus,
    BackendError, RunContext, ToolPolicy, Artifact, LogRef, McpEndpoint,
};
pub use luft_core::contract::event::{AgentEvent, RunStatus};
pub use luft_core::contract::ids::{RunId, AgentId, PhaseId, TokenUsage};
pub use luft_core::contract::finding::Finding;
pub use luft_core::scheduler::{Scheduler, SchedulerConfig, BackendRegistry, RetryPolicy};
pub use luft_core::journal::JournalStore;
pub use luft_core::state::{RunCheckpoint, CheckpointStatus};
pub use luft_runtime::{Runtime, ExecLimits, ScriptError, validate};
pub use luft_planner::{plan_workflow, PlannerConfig, PlannedWorkflow};
pub use crate::builder::{Luft, LuftBuilder, RunHandle, RunOutcome};
pub use crate::error::LuftError;
pub use luft_service::query::{StatusOutput, ReportStatus};
```

---

## 6. 使用示例

### 6.1 下游嵌入（完整 Builder）

```rust
use luft::prelude::*;

#[tokio::main]
async fn main() -> Result<(), LuftError> {
    let luft = Luft::builder()
        .backend(MyBackend::new())
        .base_dir("./runs")
        .concurrency(8)
        .build()?;

    // 异步：启动 + 监听事件
    let handle = luft.start_nl("research AI trends").await?;
    let mut rx = handle.subscribe();
    tokio::spawn(async move {
        while let Ok(evt) = rx.recv().await {
            println!("{:?}", evt);
        }
    });
    let outcome = handle.join().await?;
    println!("{}: {:?}", outcome.run_dir_name, outcome.result);

    // 同步便捷
    let outcome = luft.run_script("report({ok=true})").await?;

    // 查询
    let status = luft.status(&outcome.run_dir_name)?;
    Ok(())
}
```

### 6.2 只实现 Backend（轻量依赖）

```toml
[dependencies]
luft-core = { version = "0.2", features = ["testing"] }
```

```rust
use luft_core::contract::backend::{
    AgentBackend, AgentTask, AgentResult, AgentCapabilities,
    RunContext, BackendError,
};

pub struct MyBackend;

#[async_trait::async_trait]
impl AgentBackend for MyBackend {
    fn id(&self) -> &'static str { "mine" }
    fn capabilities(&self) -> AgentCapabilities { AgentCapabilities::default() }
    async fn run(&self, task: AgentTask, ctx: RunContext) -> Result<AgentResult, BackendError> {
        // ...
    }
}
```

### 6.3 IntoFuture 用法

```rust
let handle = luft.start_script("report({ok=true})").await?;
let outcome = handle.await?;  // 等价于 handle.join().await?
```

---

## 7. 迁移策略

分 4 步，每步可独立编译 + 测试：

### Step 1 — 创建 workspace 骨架

- 根 `Cargo.toml` 改为 `[workspace]`，members 列出所有子 crate
- 创建 `crates/` 目录 + 各子 crate 的 `Cargo.toml`（空 `src/lib.rs`）
- 原 `src/` 暂时保留不动（CLI binary 指向旧路径）

### Step 2 — 搬 luft-core

- 移动 `src/core/` → `crates/luft-core/src/`
- 替换 `use crate::core::` → crate 内 `use crate::`（对内部引用）或 `use luft_core::`（对外部消费者）
- 跑 `cargo test -p luft-core`

### Step 3 — 搬 storage / runtime / adapters / planner

- 逐个移动，每搬一个跑一次 `cargo test -p <crate>`
- 更新 `use crate::xxx` 路径为 `use luft_xxx::`

### Step 4 — 实现 Builder API + service 层 + error

- 移动 `src/service/` → `crates/luft-service/src/`
- 创建 `crates/luft/src/`（builder.rs, error.rs, prelude.rs, mcp.rs）
- 移动 `src/mcp.rs` → `crates/luft/src/mcp.rs`
- 移动 `src/mock_gen.rs` → `crates/luft-core/src/mock_gen.rs`（`#[cfg(feature="testing")]`）
- 更新 CLI (`src/main.rs`) 依赖路径
- 全量 `cargo test`

---

## 8. 决策记录

| 决策点 | 选择 | 理由 |
|--------|------|------|
| Builder 事件模型 | RunHandle.subscribe() | 每次调用返回新 Receiver，支持多消费者 |
| service 层位置 | 独立 `luft-service` crate | 保持聚合 crate 只做 re-export + Builder |
| 错误类型 | `LuftError` 仅在聚合层 | service 内部用 anyhow，聚合层做 typed 转换 |
| RunHandle 可 await | `impl IntoFuture` | 符合 Rust 惯例，`handle.await?` 一行搞定 |
| 版本策略 | 统一版本 0.2.0 | 简单，所有 crate 同步发布 |
| mock_gen 位置 | luft-core `testing` feature | 下游测试需要 MockBackend |
| mcp 位置 | 聚合 luft crate | mcp 依赖多个子 crate，不适合放进任何单一子 crate |
| feature gating | 不做复杂 feature | 子 crate 拆分本身就是隔离手段 |
