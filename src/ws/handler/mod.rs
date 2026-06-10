//! WebSocket handler 模块 — 消息分发与协议适配层。
//!
//! 架构概览:
//!
//! ```text
//!  WebSocket client
//!       │
//!       ▼
//!  routes.rs          Axum 路由 (ws_handler / health_handler)
//!       │
//!       ▼
//!  connection.rs      WS 连接生命周期管理 (读/写/超时)
//!       │
//!       ▼
//!  dispatch.rs        消息路由 (ClientMsg → 对应 handler 函数)
//!       │
//!  ┌────┼────────────────┐
//!  ▼    ▼                ▼
//! query.rs  run.rs     sub.rs
//!  (查询)   (运行)     (订阅)
//!  │        │          │
//!  ▼        ▼          ▼
//! service::query  service::run  (直接访问 registry)
//! ```
//!
//! 依赖关系:
//! - handler 层**不包含业务逻辑**，只做协议适配（解析消息 → 调 service → 格式化响应）
//! - 业务逻辑全部在 `crate::service` 中，handler 通过 service 层间接访问 `core`
//! - `AppState` 持有 backend / registry / base_dir 等共享状态
//!
//! 各文件职责:
//! - `mod.rs`          — 模块声明与 re-export
//! - `state.rs`        — AppState / Subscription 数据结构
//! - `routes.rs`       — Axum 路由入口
//! - `connection.rs`   — WS 连接生命周期（读消息 → dispatch → 写响应）
//! - `dispatch.rs`     — ClientMsg 枚举匹配 → 分发到对应 handler
//! - `query.rs`        — 查询类 handler (status / list / logs / findings / report)
//! - `run.rs`          — 运行类 handler (run / confirm / resume / cancel)
//! - `sub.rs`          — 订阅类 handler (subscribe / unsubscribe / timeout 清理)
//! - `subscription.rs` — 事件轮询 + 过滤

mod connection;
mod dispatch;
mod query;
mod routes;
mod run;
mod state;
mod sub;
mod subscription;

pub use routes::{health_handler, ws_handler};
pub use state::AppState;
pub(crate) use state::Subscription;
pub(crate) use sub::resolve_run_error;