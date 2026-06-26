# Maestro Web UI 后端开发方案

> 状态：草案 v0.1 | 更新：2025-08-19
>
> 配套文档：
> - [Web UI 设计文档](./web-ui-design.md) — 视觉风格、布局、交互
> - [Web UI 前端开发方案](./web-ui-frontend-plan.md) — 前端技术栈、组件、实现
>
> 本文档定义后端 HTTP/WS 服务的技术栈、项目结构、API 契约和实现计划。
> 前端方案见独立文档。

---

## 目录

1. [整体架构](#1-整体架构)
2. [后端技术栈](#2-后端技术栈)
3. [后端项目结构](#3-后端项目结构)
4. [API 契约（前后端共享）](#4-api-契约前后端共享)
5. [WebSocket 事件流](#5-websocket-事件流)
6. [AppState 设计](#6-appstate-设计)
7. [错误处理](#7-错误处理)
8. [分阶段实现计划](#8-分阶段实现计划)
9. [生产打包](#9-生产打包)
10. [风险与对策](#10-风险与对策)

---

## 1. 整体架构

### 1.1 前后端协作

```
开发模式：
  Browser ──▶ Vite dev (5173) ──proxy──▶ maestro serve (3000)
                                          ├── HTTP /api/*
                                          └── WS /ws/*

生产模式：
  Browser ──▶ maestro serve (3000)
               ├── 静态资源（rust-embed 嵌入 web/dist/）
               ├── HTTP /api/*
               └── WS /ws/*
```

- 开发时：Vite dev server 代理 `/api` 和 `/ws` 到后端，前端享受 HMR
- 生产时：`web/dist/` 嵌入二进制（`rust-embed`），`maestro serve` 同时服务静态资源和 API

### 1.2 服务层复用

后端直接复用现有 `service` 模块，不重写业务逻辑：

```
HTTP Handler ──▶ service::query ──▶ core/state.rs (RunStore)
                                      core/journal.rs (JournalStore)

WS Handler   ──▶ EventSender.subscribe() ──▶ broadcast channel
```

| 端点 | service 函数 | 核心模块 |
|------|-------------|----------|
| Run 列表 | `query::list_runs()` | `core/state.rs` |
| Run 详情 | `query::get_checkpoint()` | `core/state.rs` |
| Run 事件历史 | `query::get_events()` | `core/journal.rs` |
| Run 日志 | `query::get_logs()` | `core/journal.rs` |
| Run Findings | `query::get_findings()` | `core/state.rs` |
| 取消 Run | `query::cancel_run()` | `core/state.rs` |
| 发起 Run | `run::run_workflow()` → `run::execute()` | `runtime/sandbox.rs` |
| 实时事件 | `EventSender.subscribe()` | `core/contract/event.rs` |

---

## 2. 后端技术栈

### 2.1 新增依赖

| 技术 | 版本 | 用途 | 选型理由 |
|------|------|------|----------|
| **axum** | 0.8 | HTTP/WebSocket 服务器 | tokio 生态原生，Tower 中间件，与现有 tokio 无缝集成 |
| **tower-http** | 0.6 | HTTP 中间件 | CORS、静态文件服务、Gzip 压缩 |
| **rust-embed** | 8 | 静态资源嵌入 | Phase 6 才需要，将 `web/dist/` 编入二进制 |
| **tokio** | 1（已有） | 异步运行时 | 已在 Cargo.toml，`features=["full"]` 含 WS |

> 现有依赖已覆盖 `serde` / `serde_json` / `uuid` / `chrono` / `dashmap`，无需额外序列化或并发库。

### 2.2 Cargo.toml 变更

```toml
[dependencies]
# 现有依赖保持不变 ...
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v7", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
dashmap = "6"
# ...

# 新增
axum = { version = "0.8", features = ["ws"] }
tower-http = { version = "0.6", features = ["cors", "fs", "compression-gzip"] }
rust-embed = "8"  # Phase 6
```

---

## 3. 后端项目结构

新增 `src/server/` 模块和 `serve` 子命令，不修改现有代码。

```
src/
├── main.rs                      # 修改：添加 Serve 变体到 Commands enum
├── commands/
│   ├── mod.rs                   # 修改：pub mod serve;
│   └── serve.rs                 # 新增：maestro serve --port 3000
├── server/                      # 新增：HTTP/WS 服务器
│   ├── mod.rs                   # AppState + 路由注册 + CORS + 启动
│   ├── error.rs                 # ApiError → HTTP 响应映射
│   └── routes/
│       ├── mod.rs               # 路由聚合
│       ├── runs.rs              # Run CRUD + 发起 + 取消
│       ├── workflows.rs         # Workflow CRUD
│       ├── backends.rs          # Backend 查询/配置
│       ├── stats.rs             # Dashboard 统计聚合
│       └── events.rs            # WebSocket 事件流（per-run + dashboard）
├── service/                     # 现有，不修改
├── core/                        # 现有，不修改
├── runtime/                     # 现有，不修改
└── ...
```

### 3.1 serve 子命令

```rust
// src/commands/serve.rs
use clap::Args;

#[derive(Args)]
pub struct ServeArgs {
    /// 监听端口
    #[arg(long, default_value = "3000")]
    pub port: u16,

    /// 监听地址
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
}

pub async fn run(args: ServeArgs) -> anyhow::Result<()> {
    let app = maestro::server::create_app(/* ... */)?;
    let addr = format!("{}:{}", args.host, args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

### 3.2 main.rs 修改

```rust
// src/main.rs — Commands enum 新增
#[derive(Debug, Subcommand)]
enum Commands {
    // ... 现有命令 ...
    Serve(commands::serve::ServeArgs),  // 新增
}
```

---

## 4. API 契约（前后端共享）

> 此节是前后端共享的 API 契约定义。前端 `api/types.ts` 和后端 handler 必须对齐。

### 4.1 端点总览

所有 HTTP 前缀 `/api`，WebSocket 路径 `/ws`。

| 方法 | 路径 | 说明 | service 函数 |
|------|------|------|-------------|
| GET | `/api/health` | 健康检查 | — |
| GET | `/api/runs` | Run 列表 | `query::list_runs()` |
| POST | `/api/runs` | 发起新 Run | `run::run_workflow()` + `tokio::spawn` |
| GET | `/api/runs/:id` | Run 详情（checkpoint） | `query::get_checkpoint()` |
| GET | `/api/runs/:id/status` | Run 状态 | `query::get_status()` |
| GET | `/api/runs/:id/events` | Run 事件历史 | `query::get_events()` |
| GET | `/api/runs/:id/logs` | Run 日志 | `query::get_logs()` |
| GET | `/api/runs/:id/findings` | Run Findings | `query::get_findings()` |
| POST | `/api/runs/:id/cancel` | 取消 Run | `CancellationToken` |
| GET | `/api/workflows` | Workflow 列表 | 文件系统扫描 |
| GET | `/api/workflows/:name` | Workflow 内容 | 读取 .lua |
| PUT | `/api/workflows/:name` | 保存 Workflow | 写入 .lua |
| POST | `/api/workflows` | 新建 Workflow | 创建 .lua |
| GET | `/api/backends` | Backend 列表 | 配置读取 |
| PUT | `/api/backends/:id` | 更新 Backend | 配置写入 |
| GET | `/api/stats` | Dashboard 统计 | 聚合查询 |
| WS | `/ws/runs/:id` | Run 实时事件流 | `EventSender.subscribe()` |
| WS | `/ws/dashboard` | 全局事件流 | 多 run 聚合 |

### 4.2 数据格式

#### POST `/api/runs` — 发起 Run

```json
// Request
{
  "workflow": "code-review",
  "task": "分析 src/ 目录代码质量",
  "backend": "claude-sonnet-4",
  "args": {}
}

// Response 201
{
  "run_id": "0192a3f4-b5c6-7d8e-9f01-0a1b2c3d4e5f",
  "run_dir": "2025-08-19_0192a3f4",
  "status": "running",
  "ws_url": "/ws/runs/0192a3f4-b5c6-7d8e-9f01-0a1b2c3d4e5f"
}
```

#### GET `/api/runs` — Run 列表

查询参数：`?status=running&limit=20&offset=0&q=keyword`

```json
{
  "runs": [
    {
      "run_id": "0192a3f4-...",
      "run_dir": "2025-08-19_0192a3f4",
      "task": "分析代码质量",
      "status": "running",
      "current_phase": 2,
      "total_phases": 3,
      "total_tokens": 2600,
      "started_at": "2025-08-19T12:03:01Z",
      "elapsed_ms": 192000
    }
  ],
  "total": 12
}
```

#### GET `/api/runs/:id` — Run 详情

```json
{
  "checkpoint": {
    "run_id": "0192a3f4-...",
    "task": "分析代码质量",
    "status": "running",
    "current_phase": 2,
    "completed_phases": [
      { "phase_id": 1, "label": "生成分析", "ok": 3, "failed": 0 }
    ],
    "agent_results": {
      "agent_1": {
        "status": "done",
        "tokens": { "input": 800, "output": 400 },
        "elapsed_ms": 12000
      }
    },
    "findings": [],
    "total_tokens": 2600
  }
}
```

#### GET `/api/stats` — Dashboard 统计

```json
{
  "today_runs": 23,
  "today_tokens": 142000,
  "today_success": 14,
  "today_failed": 3,
  "active_runs": [ /* RunSummary[] — 运行中的 */ ],
  "recent_runs": [ /* RunSummary[] — 最近 10 条 */ ]
}
```

### 4.3 序列化约定

| 约定 | 说明 |
|------|------|
| 时间格式 | ISO 8601 字符串（`chrono` serde 默认） |
| ID 格式 | UUID v7 字符串 |
| 事件序列化 | `AgentEvent` serde enum，tag = variant 名 |
| 错误格式 | `{ "error": "message", "code": "optional" }` |
| 分页 | `?limit=20&offset=0`，响应含 `total` |

> **关键：** `AgentEvent` 已在 `src/core/contract/event.rs:15` 派生 `Serialize`/`Deserialize`，
> JSON 格式直接复用 serde 序列化输出，无需额外转换层。

---

## 5. WebSocket 事件流

### 5.1 连接生命周期

```
Client                          Server
  │                               │
  │── WS Connect /ws/runs/:id ──▶│
  │                               │── 查找 AppState.active_runs[run_id]
  │                               │── EventSender.subscribe()
  │◀── 历史事件回放 ──────────────│  （从 events.jsonl 读取）
  │◀── 实时事件转发 ──────────────│  （从 broadcast channel 接收）
  │◀── ping (30s) ───────────────│
  │                               │
  │── Run Done ─────────────────▶│
  │◀── { type: "RunDone" } ──────│
  │◀── WS Close ─────────────────│
  │                               │
```

### 5.2 事件消息格式

每条 WebSocket 消息是一个 JSON 序列化的 `AgentEvent`：

```json
{ "type": "RunStarted", "run_id": "...", "task": "...", "ts": "..." }
{ "type": "PhaseStarted", "run_id": "...", "phase_id": 1, "label": "生成分析", "planned": 3 }
{ "type": "AgentStarted", "run_id": "...", "phase_id": 1, "agent_id": "agent_1", "prompt_preview": "...", "model": "claude-sonnet-4" }
{ "type": "AgentProgress", "run_id": "...", "agent_id": "agent_1", "delta": { ... } }
{ "type": "AgentDone", "run_id": "...", "agent_id": "agent_1", "status": "done", "tokens": { ... }, "elapsed_ms": 12000 }
{ "type": "PhaseDone", "run_id": "...", "phase_id": 1, "ok": 3, "failed": 0 }
{ "type": "RunDone", "run_id": "...", "status": "completed", "total_tokens": { ... }, "report": { ... } }
```

### 5.3 心跳与超时

| 机制 | 间隔 | 说明 |
|------|------|------|
| 服务端 ping | 30s | 发送 `{"type":"ping"}` |
| 客户端超时 | 60s | 无消息则断开重连 |
| Run 完成 | — | 发送 `RunDone` 后服务端关闭连接 |

### 5.4 实现要点

```rust
// src/server/routes/events.rs
async fn run_events_ws(
    ws: WebSocketUpgrade,
    Path(run_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    // 1. 查找活跃 Run 的 sender
    let sender = state.active_runs
        .get(&run_id)
        .map(|e| e.value().clone())
        .ok_or(ApiError::NotFound)?;

    // 2. 订阅 broadcast
    let mut rx = sender.subscribe();

    // 3. 读取历史事件
    let history = maestro::service::query::get_events(&run_id, &state.base_dir)?;

    // 4. 升级 WebSocket，开始转发
    Ok(ws.on_upgrade(move |socket| async move {
        let mut sender = socket;
        // 先发送历史
        for event in history {
            let msg = serde_json::to_string(&event).unwrap();
            let _ = sender.send(Message::Text(msg)).await;
        }
        // 再转发实时
        while let Ok(event) = rx.recv().await {
            let msg = serde_json::to_string(&event).unwrap();
            if sender.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
    }))
}
```

### 5.5 WS /ws/dashboard — 全局事件流

订阅所有活跃 Run 的事件。实现方式：
- 维护一个全局 `EventSender`（dashboard channel）
- 每次发起 Run 时，`tokio::spawn` 一个转发任务，将 per-run 事件复制到 dashboard channel
- 客户端订阅 dashboard channel 即可收到所有 Run 的事件

---

## 6. AppState 设计

```rust
// src/server/mod.rs
use std::sync::Arc;
use std::path::PathBuf;
use dashmap::DashMap;
use tokio::sync::{broadcast, RwLock};
use tokio_util::sync::CancellationToken;

use maestro::core::contract::event::EventSender;
use maestro::core::contract::RunId;

pub struct AppState {
    /// 活跃 Run 的事件广播通道
    /// key = run_id, value = EventSender
    pub active_runs: DashMap<RunId, EventSender>,

    /// Run 句柄（用于取消）
    pub run_handles: DashMap<RunId, CancellationToken>,

    /// 全局 dashboard 事件流
    pub dashboard_tx: EventSender,

    /// 数据目录（runs 存储根目录）
    pub base_dir: PathBuf,

    /// Workflow 脚本目录
    pub workflows_dir: PathBuf,

    /// Backend 配置（读写锁，支持热更新）
    pub backends_config: RwLock<BackendsConfig>,
}

impl AppState {
    pub fn new(base_dir: PathBuf, workflows_dir: PathBuf) -> Self {
        let (dashboard_tx, _) = broadcast::channel(256);
        Self {
            active_runs: DashMap::new(),
            run_handles: DashMap::new(),
            dashboard_tx,
            base_dir,
            workflows_dir,
            backends_config: RwLock::new(BackendsConfig::load()),
        }
    }

    /// 注册一个活跃 Run
    pub fn register_run(&self, run_id: RunId) -> EventSender {
        let (tx, _) = broadcast::channel(256);
        self.active_runs.insert(run_id.clone(), tx.clone());
        tx
    }

    /// Run 完成后清理
    pub fn unregister_run(&self, run_id: &RunId) {
        self.active_runs.remove(run_id);
        self.run_handles.remove(run_id);
    }
}
```

### 6.1 发起 Run 的处理流程

```rust
// src/server/routes/runs.rs
async fn start_run(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StartRunRequest>,
) -> Result<StatusCode, ApiError> {
    // 1. 读取 Workflow 脚本
    let script = std::fs::read_to_string(
        state.workflows_dir.join(format!("{}.lua", req.workflow))
    ).map_err(|_| ApiError::NotFound)?;

    // 2. 构建 RunContext
    let run_id = RunId::new_v7();
    let cancel_token = CancellationToken::new();
    let event_tx = state.register_run(run_id.clone());

    // 3. 保存 cancel token
    state.run_handles.insert(run_id.clone(), cancel_token.clone());

    // 4. 转发到 dashboard channel
    let dashboard_tx = state.dashboard_tx.clone();
    let rid = run_id.clone();
    tokio::spawn(async move {
        let mut rx = event_tx.subscribe();
        while let Ok(event) = rx.recv().await {
            let _ = dashboard_tx.send(event);
        }
    });

    // 5. 后台执行 Run
    let state2 = state.clone();
    let rid2 = run_id.clone();
    tokio::spawn(async move {
        let ctx = RunContext {
            run_id: rid2.clone(),
            cancel: cancel_token,
            events: event_tx,
        };
        let result = maestro::service::run::execute(&ctx, rt, script).await;
        state2.unregister_run(&rid2);
        // Run 结果通过 RunDone 事件传达
    });

    // 6. 立即返回
    Ok((StatusCode::CREATED, Json(json!({
        "run_id": run_id.to_string(),
        "status": "running",
        "ws_url": format!("/ws/runs/{}", run_id),
    }))))
}
```

---

## 7. 错误处理

### 7.1 统一错误类型

```rust
// src/server/error.rs
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        (status, Json(json!({ "error": message }))).into_response()
    }
}
```

### 7.2 状态码映射

| 错误 | HTTP | 场景 |
|------|------|------|
| Run/Workflow 不存在 | 404 | `GET /api/runs/:id` 找不到 |
| 参数错误 | 400 | `POST /api/runs` 缺少必填字段 |
| Run 已在运行 | 409 | 重复发起相同 run_id |
| 内部错误 | 500 | service 层返回 Err |

### 7.3 CORS 配置

```rust
// src/server/mod.rs
use tower_http::cors::{CorsLayer, Any};

fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)          // 开发模式：允许所有 origin
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(Any)
}
```

> 生产模式下可收紧为 `http://localhost:3000`，开发模式允许 Vite 的 5173 端口。

---

## 8. 分阶段实现计划

### Phase 0：服务器骨架（1 天）

**目标**：`maestro serve` 启动，健康检查可用。

**任务：**
- `Cargo.toml` 添加 `axum`、`tower-http`
- 实现 `src/server/mod.rs`：`create_app()`、路由注册、CORS
- 实现 `src/server/error.rs`：`ApiError`
- 实现 `src/commands/serve.rs`：子命令解析
- 修改 `src/main.rs`：添加 `Serve` 变体
- 实现 `GET /api/health` → `{ "status": "ok" }`

**验收：**
- `cargo run -- serve` 启动 HTTP 服务
- `curl localhost:3000/api/health` 返回 OK

---

### Phase 1：只读 API（2 天）

**目标**：前端能查询 Run 列表和详情。

**任务：**
- `GET /api/runs` → `query::list_runs()`
- `GET /api/runs/:id` → `query::get_checkpoint()`
- `GET /api/runs/:id/events` → `query::get_events()`
- `GET /api/runs/:id/findings` → `query::get_findings()`
- `GET /api/runs/:id/logs` → `query::get_logs()`
- 分页和筛选参数处理

**验收：**
- 所有 GET 端点返回正确数据
- 前端 Phase 1（Runs 列表）能成功对接

---

### Phase 2：WebSocket + 发起 Run（2-3 天）

**目标**：实时事件推送 + 发起新 Run。

**任务：**
- 实现 `AppState`（`active_runs`、`run_handles`、`dashboard_tx`）
- `WS /ws/runs/:id`：订阅 `EventSender`，先回放历史再转发实时
- `POST /api/runs`：发起 Run（`tokio::spawn` + 注册到 AppState）
- `POST /api/runs/:id/cancel`：通过 `CancellationToken` 取消
- `WS /ws/dashboard`：全局事件流
- 心跳机制（30s ping）
- Run 完成后清理 AppState

**验收：**
- 前端发起 Run 后能实时收到事件
- Run 完成后 WS 自动关闭
- 取消 Run 能正常中断

---

### Phase 3：Dashboard + Workflow + Backend API（2-3 天）

**目标**：支撑前端 Phase 3-5 的功能。

**任务：**
- `GET /api/stats`：聚合统计查询
- `GET /api/workflows`：扫描 .lua 文件列表
- `GET /api/workflows/:name`：读取 .lua 内容
- `PUT /api/workflows/:name`：保存 .lua
- `POST /api/workflows`：新建
- `GET /api/backends`：读取 backend 配置
- `PUT /api/backends/:id`：更新配置

**验收：**
- Dashboard 数据正确
- Workflow 读写正常
- Backend 配置可查询和更新

---

### Phase 4：生产打包（1 天）

**目标**：单二进制部署。

**任务：**
- `rust-embed` 嵌入 `web/dist/`
- axum 静态文件路由（SPA fallback → `index.html`）
- `maestro serve` 一个命令启动完整应用

**验收：**
- `cargo build --release` 产出单二进制
- 运行二进制即可访问完整 Web UI

---

### 工期汇总

| Phase | 内容 | 预估工时 |
|-------|------|----------|
| 0 | 服务器骨架 | 1 天 |
| 1 | 只读 API | 2 天 |
| 2 | WebSocket + 发起 Run | 2-3 天 |
| 3 | Dashboard + Workflow + Backend | 2-3 天 |
| 4 | 生产打包 | 1 天 |
| **合计** | | **8-10 天** |

---

## 9. 生产打包

### 9.1 静态资源嵌入

```rust
// src/server/mod.rs
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct WebAssets;

/// 静态文件服务（SPA fallback）
fn static_routes() -> Router {
    Router::new()
        .route("/", get serve_index)
        .route("/assets/*path", get(serve_asset))
        .fallback(serve_index) // SPA fallback → index.html
}

async fn serve_index() -> impl IntoResponse {
    let asset = WebAssets::get("index.html").unwrap();
    (
        [(header::CONTENT_TYPE, "text/html")],
        asset.data,
    )
}

async fn serve_asset(Path(path): Path<String>) -> impl IntoResponse {
    match WebAssets::get(&path) {
        Some(asset) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], asset.data).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
```

### 9.2 构建流程

```bash
# 完整构建
cd web && npm run build       # → web/dist/
cd .. && cargo build --release

# 运行
./target/release/maestro serve --port 3000
```

### 9.3 CI 配置（参考）

```yaml
build-web:
  steps:
    - cd web && npm ci && npm run build
    - cargo build --release
```

---

## 10. 风险与对策

| 风险 | 影响 | 对策 |
|------|------|------|
| `AgentEvent` serde 格式与前端类型不一致 | 运行时解析错误 | Phase 1 初期编写集成测试，验证 `serde_json::to_string(&event)` 输出 |
| broadcast channel 容量不足（256） | 高频 AgentProgress 事件丢失 | 监控 lag；前端从 `GET /api/runs/:id/events` 回补 |
| `tokio::spawn` 的 Run 未 panic-safe | 服务崩溃 | spawn 内包裹 `catch_unwind` 或返回 `RunDone(failed)` |
| 多 Run 并发 WS 连接 | 内存增长 | Run 完成后 `unregister_run()` 清理 sender；设置 WS 空闲超时 |
| Rust 二进制体积增大 | ~5-10MB（嵌入前端） | 可接受，开发者工具不做极致体积优化 |
| Workflow 路径遍历攻击 | 读取/写入任意文件 | `workflows/:name` 参数校验（禁止 `..`、绝对路径） |
