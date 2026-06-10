# Service Layer Extraction — 方案设计

> 从 WS handler / CLI 中抽取业务逻辑为独立 service 层，使 handler 成为薄适配层。

## 1. 现状分析

### 1.1 当前调用关系

```
┌─────────────┐     ┌──────────────────────┐
│  CLI (main) │────→│  cli::run()          │──→ runtime / scheduler
│             │     │  cli::status_cmd()   │──→ core::state
│             │     │  cli::list_runs_cmd()│──→ core::state
│             │     │  cli::logs_cmd()     │──→ core::state
│             │     │  cli::findings_cmd() │──→ core::state
│             │     │  cli::cancel_cmd()   │──→ core::state
└─────────────┘     └──────────────────────┘

┌─────────────┐     ┌──────────────────────┐     ┌───────────────┐
│  WS Client  │──WS→│  ws/handler/         │     │               │
│             │     │  dispatch.rs         │────→│  cli::*_cmd() │ (query 类)
│             │     │    ├── query.rs      │     │               │
│             │     │    ├── run.rs        │──→  planner + cli::run() (业务逻辑内联)
│             │     │    └── sub.rs        │     │               │
└─────────────┘     └──────────────────────┘     └───────────────┘
```

### 1.2 问题

| 问题 | 位置 | 影响 |
|------|------|------|
| **业务逻辑内联在 WS handler** | `ws/handler/run.rs` ~156 行 | 无法被 CLI/TUI/test 直接复用 |
| **query handler 间接调 CLI** | `ws/handler/query.rs` → `cli::*_cmd()` | 依赖方向反了（传输层不应依赖 CLI 层） |
| **handler 签名耦合协议** | 所有 handler 接收 `mpsc::Sender<ServerMsg>` | 无法脱离 WebSocket 环境调用 |
| **run handler 混合职责** | `handle_run` 同时做验证+规划+注册+启动 | 难以单独测试某个环节 |
| **TUI 无法调 WS 功能** | `cli::run()` 和 WS handler 各管各 | TUI 无法订阅事件、管理并发 run |

### 1.3 现有代码职责清单

| 文件 | 行数 | 职责 | 协议耦合度 |
|------|------|------|-----------|
| `ws/handler/dispatch.rs` | ~75 | 消息路由 | 低（纯 match） |
| `ws/handler/run.rs` | ~156 | run 生命周期 | **高**（out_tx, subscriptions） |
| `ws/handler/query.rs` | ~120 | 查询代理到 cli | 中（out_tx 响应） |
| `ws/handler/sub.rs` | ~70 | 订阅管理 | 高（out_tx, registry） |
| `ws/handler/connection.rs` | ~100 | WS 连接生命周期 | 高（WebSocket 专属） |
| `cli.rs` | ~450 | CLI 入口 + 查询函数 + run 编排 | 低（部分有 println） |

## 2. 目标架构

```
┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐
│   CLI   │  │   TUI   │  │  WS     │  │  Tests  │
│  main   │  │ (未来)  │  │ handler │  │  (unit) │
└────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘
     │            │            │            │
     └────────────┴────────────┴────────────┘
                         │
              ┌──────────▼──────────┐
              │    service layer    │   ← 新增，纯业务逻辑
              │                     │
              │  RunService         │
              │  QueryService       │
              │  SubscribeService   │
              └──────────┬──────────┘
                         │
              ┌──────────▼──────────┐
              │    core / runtime   │   ← 现有，不变
              └─────────────────────┘
```

**核心原则**：
- Service 函数签名**只接受业务参数**，不出现 `mpsc::Sender`、`ServerMsg`、`WebSocket` 等
- Service 返回 `Result<T>` 或领域事件，由调用方决定如何呈现
- Handler / CLI / Test 都是 service 的**消费者**，平级关系
- 依赖方向：`ws` → `service` ← `cli`，`service` → `core`

## 3. 模块设计

### 3.1 目录结构

```
src/
├── service/              ← 新增
│   ├── mod.rs
│   ├── run.rs            ← run 生命周期管理
│   ├── query.rs          ← 查询操作
│   └── subscribe.rs      ← 订阅/事件管理
├── ws/
│   └── handler/
│       ├── dispatch.rs   ← 不变（路由）
│       ├── run.rs        ← 瘦身：解析 → 调 service → 格式化响应
│       ├── query.rs      ← 瘦身
│       ├── sub.rs        ← 瘦身
│       ├── connection.rs ← 不变
│       └── state.rs      ← AppState 保留（含 service 依赖）
├── cli.rs                ← 瘦身：CLI 特有的 println/交互 保留，业务逻辑调 service
└── core/                 ← 不变
```

### 3.2 Service 接口设计

#### QueryService

```rust
// src/service/query.rs

use crate::core::contract::finding::Finding;
use crate::core::contract::ids::RunId;
use crate::cli::StatusOutput;
use anyhow::Result;
use std::path::Path;

pub fn list_runs(base_dir: &Path) -> Result<Vec<StatusOutput>> {
    // 从 cli::list_runs_cmd 迁移，去掉 PathBuf 参数改为 &Path
}

pub fn get_status(run_id: RunId, base_dir: &Path) -> Result<Option<StatusOutput>> {
    // 从 cli::status_cmd 迁移
}

pub fn get_logs(run_id: RunId, base_dir: &Path, limit: Option<usize>) -> Result<Vec<String>> {
    // 从 cli::logs_cmd 迁移
}

pub fn get_findings(run_id: RunId, base_dir: &Path) -> Result<Vec<Finding>> {
    // 从 cli::findings_cmd 迁移
}

pub fn get_report(run_id: RunId, base_dir: &Path) -> Result<Option<serde_json::Value>> {
    // 从 query.rs handle_get_report 中提取（当前该逻辑在 WS handler 独有）
}
```

#### RunService

```rust
// src/service/run.rs

use crate::core::contract::backend::AgentBackend;
use crate::core::contract::event::AgentEvent;
use crate::core::contract::ids::RunId;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub struct RunPlan {
    pub run_id: RunId,
    pub script: String,
}

pub struct RunHandle {
    pub events: broadcast::Sender<AgentEvent>,
    pub cancel: CancellationToken,
    pub task: tokio::task::JoinHandle<()>,
}

pub enum RunPrepareOutcome {
    NeedConfirm { run_id: RunId, script: String },
    Ready(RunPlan),
}

pub struct RunParams {
    pub nl: Option<String>,
    pub workflow: Option<std::path::PathBuf>,
    pub script: Option<String>,
    pub args: serde_json::Value,
    pub confirm: bool,
    pub base_dir: std::path::PathBuf,
}

pub async fn prepare_run(
    params: RunParams,
    backend: Arc<dyn AgentBackend>,
) -> Result<RunPrepareOutcome> {
    // 验证（exactly one of nl/workflow/script）
    // 获取 semaphore permit（由调用方传入或内部管理）
    // 生成 run_id
    // 如果 confirm + nl：plan_workflow → NeedConfirm
    // 否则：resolve script → Ready
}

pub async fn execute_run(
    run_id: RunId,
    script: String,
    backend: Arc<dyn AgentBackend>,
    base_dir: &Path,
    args: serde_json::Value,
) -> Result<(broadcast::Sender<AgentEvent>, CancellationToken, tokio::task::JoinHandle<()>)> {
    // 创建 broadcast channel + cancel token
    // 构建 RunArgs + 调用 cli::run()（或直接编排 scheduler + runtime）
    // spawn 后台任务
    // 返回 handle
}
```

#### SubscribeService

```rust
// src/service/subscribe.rs

use crate::core::contract::event::AgentEvent;
use crate::core::contract::ids::RunId;
use anyhow::Result;
use tokio::sync::broadcast;

pub struct SubscriptionOpts {
    pub filter: Option<Vec<String>>,
}

pub fn subscribe(
    run_id: RunId,
    registry: &crate::ws::registry::RunRegistry,
    opts: SubscriptionOpts,
) -> Result<broadcast::Receiver<AgentEvent>> {
    // 从 registry 获取 receiver
    // 如果 run 不活跃，检查是否已完成 vs 不存在
}

pub fn cancel_pending_confirm(
    run_id: RunId,
    pending: &mut std::collections::HashMap<RunId, (String, std::time::Instant)>,
) -> Option<String> {
    // 取出 pending confirm
}
```

### 3.3 Handler 重构后示例

**重构前** (`ws/handler/query.rs`):

```rust
pub async fn handle_get_status(
    req_id: String,
    run_id: RunId,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
) {
    match cli::status_cmd(run_id, &state.base_dir) {
        Ok(Some(status)) => {
            let _ = out_tx.send(ServerMsg::Status { req_id, run_id, data: status }).await;
        }
        Ok(None) => {
            let _ = out_tx.send(ServerMsg::Error {
                req_id, code: ErrorCode::NotFound,
                message: format!("run {} not found", run_id),
            }).await;
        }
        Err(e) => { /* ... */ }
    }
}
```

**重构后**:

```rust
pub async fn handle_get_status(
    req_id: String,
    run_id: RunId,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
) {
    let result = service::query::get_status(run_id, &state.base_dir);
    let msg = match result {
        Ok(Some(status)) => ServerMsg::Status { req_id, run_id, data: status },
        Ok(None) => ServerMsg::Error {
            req_id, code: ErrorCode::NotFound,
            message: format!("run {} not found", run_id),
        },
        Err(e) => ServerMsg::Error {
            req_id, code: ErrorCode::Internal,
            message: e.to_string(),
        },
    };
    let _ = out_tx.send(msg).await;
}
```

变化：`cli::status_cmd` → `service::query::get_status`，handler 只做结果→消息的映射。

### 3.4 CLI 重构后示例

**重构前** (`cli.rs`):

```rust
pub fn list_runs_cmd(base_dir: &PathBuf) -> Result<Vec<StatusOutput>> {
    let run_ids = list_runs(base_dir)?;
    // ... 业务逻辑 ...
}
```

**重构后**:

```rust
pub fn list_runs_cmd(base_dir: &Path) -> Result<Vec<StatusOutput>> {
    let outputs = service::query::list_runs(base_dir)?;
    for o in &outputs {
        println!("{}  {}  {}", o.run_id, o.status, o.task);
    }
    Ok(outputs)
}
```

变化：业务逻辑移入 service，CLI 只保留 `println` 等输出逻辑。

## 4. 迁移策略

### 4.1 分阶段实施

#### Phase 1：QueryService（低风险，~1h）

| 步骤 | 操作 | 涉及文件 |
|------|------|----------|
| 1.1 | 创建 `src/service/mod.rs` + `src/service/query.rs` | 新建 |
| 1.2 | 将 `cli::list_runs_cmd` / `status_cmd` / `logs_cmd` / `findings_cmd` 的核心逻辑移入 service | `cli.rs`, `service/query.rs` |
| 1.3 | CLI 函数改为调 service + 保留 println | `cli.rs` |
| 1.4 | WS query handler 改为调 service（去掉对 cli 的依赖） | `ws/handler/query.rs` |
| 1.5 | 移动现有测试到 service 层 | `cli.rs` tests → `service/query.rs` tests |

#### Phase 2：RunService（中风险，~2h）

| 步骤 | 操作 | 涉及文件 |
|------|------|----------|
| 2.1 | 创建 `src/service/run.rs` | 新建 |
| 2.2 | 从 `ws/handler/run.rs` 提取 `prepare_run` | `service/run.rs` |
| 2.3 | 从 `ws/handler/run.rs` 提取 `execute_run` | `service/run.rs` |
| 2.4 | WS run handler 改为调 service | `ws/handler/run.rs` |
| 2.5 | 处理 `handle_confirm_run` / `handle_resume` / `handle_cancel` | `service/run.rs` |
| 2.6 | 确保 CLI `run()` 也能复用 service 的 prepare 逻辑 | `cli.rs` |

#### Phase 3：SubscribeService（低风险，~0.5h）

| 步骤 | 操作 | 涉及文件 |
|------|------|----------|
| 3.1 | 创建 `src/service/subscribe.rs` | 新建 |
| 3.2 | 提取订阅/取消订阅逻辑 | `ws/handler/sub.rs` → `service/subscribe.rs` |
| 3.3 | 提取 `check_confirm_timeouts` | `service/run.rs` 或 `service/subscribe.rs` |

### 4.2 风险控制

| 风险 | 缓解措施 |
|------|----------|
| Service 函数签名变化导致多处调用方改动 | Phase 1 先做 query（签名最简单），验证模式可行 |
| `cli::run()` 内部编排逻辑复杂（scheduler + runtime + journal） | Phase 2 不强行拆 `cli::run()`，先只抽取 WS handler 的 prepare + spawn 逻辑 |
| 现有测试在迁移过程中 break | 移动而非重写测试，确保测试函数签名对齐 |
| AppState 需要调整 | AppState 保持不变，service 函数接收具体参数（base_dir, backend）而非整个 AppState |

## 5. 各模块函数迁移对照表

| 原位置 | 原函数 | 迁移到 | 新函数名 | 备注 |
|--------|--------|--------|----------|------|
| `cli.rs` | `list_runs_cmd()` | `service/query.rs` | `list_runs()` | CLI 保留 println wrapper |
| `cli.rs` | `status_cmd()` | `service/query.rs` | `get_status()` | CLI 保留 println wrapper |
| `cli.rs` | `logs_cmd()` | `service/query.rs` | `get_logs()` | CLI 保留 println wrapper |
| `cli.rs` | `findings_cmd()` | `service/query.rs` | `get_findings()` | CLI 保留 println wrapper |
| `cli.rs` | `cancel_cmd()` | `service/query.rs` | `cancel_run()` | CLI 保留 println wrapper |
| `ws/handler/query.rs` | `handle_get_report()` 内逻辑 | `service/query.rs` | `get_report()` | 新增，当前仅在 WS handler |
| `ws/handler/run.rs` | 验证 + 规划逻辑 | `service/run.rs` | `prepare_run()` | confirm 分支也在内 |
| `ws/handler/run.rs` | spawn + 注册逻辑 | `service/run.rs` | `execute_run()` | |
| `ws/handler/run.rs` | confirm 处理 | `service/run.rs` | `confirm_run()` | |
| `ws/handler/run.rs` | resume 处理 | `service/run.rs` | `resume_run()` | |
| `ws/handler/sub.rs` | subscribe 逻辑 | `service/subscribe.rs` | `subscribe()` | |
| `ws/handler/sub.rs` | unsubscribe 逻辑 | `service/subscribe.rs` | `unsubscribe()` | |
| `ws/handler/sub.rs` | check_confirm_timeouts | `service/run.rs` | `check_confirm_timeouts()` | |

## 6. 不变的模块

以下模块**不做修改**：

- `ws/handler/dispatch.rs` — 路由逻辑不变，只是调的函数从 handler 内部逻辑变为 service 调用
- `ws/handler/connection.rs` — WS 连接管理，属于传输层
- `ws/handler/routes.rs` — Axum 路由定义
- `ws/protocol.rs` — 消息协议定义
- `ws/registry.rs` — RunRegistry 实现
- `ws/handler/subscription.rs` — poll_subscriptions 是 WS 专属的事件轮询
- `ws/handler/state.rs` — AppState 结构体
- `core/*` — 完全不动
- `runtime/*` — 完全不动
- `planner/*` — 完全不动

## 7. 测试策略

### 7.1 Service 层测试

Service 函数接受 `&Path` / `RunId` 等纯参数，可直接用 tempdir 测试：

```rust
#[test]
fn service_list_runs() {
    let temp = tempfile::tempdir().unwrap();
    // 创建测试数据...
    let result = service::query::list_runs(temp.path()).unwrap();
    assert_eq!(result.len(), 2);
}
```

### 7.2 Handler 测试（可选）

重构后 handler 极薄（解析→调 service→格式化），可通过 mock service 或直接用 `mpsc::channel` 测试：

```rust
#[tokio::test]
async fn handle_get_status_found() {
    let state = /* AppState with test base_dir */;
    let (tx, mut rx) = mpsc::channel(64);
    // 准备测试数据...
    handle_get_status("req1".into(), run_id, &state, &tx).await;
    let msg = rx.try_recv().unwrap();
    assert!(matches!(msg, ServerMsg::Status { .. }));
}
```

### 7.3 测试迁移规则

| 原测试位置 | 迁移目标 |
|-----------|---------|
| `cli.rs` 中的 `list_runs_cmd_*` tests | `service/query.rs` tests（改调 service 函数） |
| `cli.rs` 中的 `status_cmd_*` tests | `service/query.rs` tests |
| `cli.rs` 中的 `logs_cmd_*` tests | `service/query.rs` tests |
| `cli.rs` 中的 `findings_cmd_*` tests | `service/query.rs` tests |
| `cli.rs` 中的 `cancel_cmd_*` tests | `service/query.rs` tests |
| `cli.rs` 中的 `status_output_*` tests | `service/query.rs` 或保留在 cli（因测试 StatusOutput 转换） |
| `cli.rs` 中的 `print_progress_*` tests | 保留在 cli（纯 UI 逻辑） |

## 8. 预期收益

| 指标 | 重构前 | 重构后 |
|------|--------|--------|
| WS handler 对 cli 的依赖 | `use crate::cli` | 无 |
| Service 可被直接调用 | 不可以（耦合 WS 协议） | 可以（任何传输层） |
| 业务逻辑测试难度 | 需启动 WS 或 mock channel | 直接调函数 + tempdir |
| TUI 复用 WS 功能 | 不可以 | 通过 service 层直接调 |
| cli.rs 中 println 与业务逻辑混合 | 混合 | 分离（业务在 service，输出在 cli） |
