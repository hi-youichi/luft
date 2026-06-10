//! WebSocket 共享状态与订阅数据结构。
//!
//! [`AppState`] 是 handler 层的核心共享状态，在 WS 连接建立时通过 Axum State 注入，
//! 被 dispatch / query / run / sub 等 handler 函数共享引用。
//!
//! [`Subscription`] 表示单个客户端对某个 run 的事件流订阅，
//! 包含可选的事件类型过滤器（`filter`）和实际的 broadcast stream。

use crate::core::contract::backend::AgentBackend;
use crate::core::contract::event::AgentEvent;
use crate::ws::registry::RunRegistry;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

/// handler 层全局共享状态，每个 WS 连接持有同一份 clone（内部用 Arc）。
///
/// 字段说明:
/// - `backend`       — LLM 后端适配器（如 OpenCode），用于 NL → Lua 规划和 run 执行
/// - `registry`      — 活跃 run 注册表（DashMap），存储 RunHandle（events / cancel / task）
/// - `base_dir`      — run 数据存储根目录（通常为 `.maestro/runs`）
/// - `run_permits`   — 并发 run 数量信号量，防止资源耗尽
/// - `confirm_timeout` — NL confirm 模式下的超时时间（超时后丢弃 pending script）
#[derive(Clone)]
pub struct AppState {
    pub backend: Arc<dyn AgentBackend>,
    pub registry: RunRegistry,
    pub base_dir: PathBuf,
    pub run_permits: Arc<tokio::sync::Semaphore>,
    pub confirm_timeout: Duration,
}

/// 单个客户端对某个 run 的事件流订阅。
///
/// - `filter`  — 可选的事件类型白名单（如 `["phase_started", "agent_done"]`），
///               `None` 表示接收所有事件
/// - `stream`  — 包装了 broadcast::Receiver 的 Stream，由 `subscription.rs` 轮询
pub(crate) struct Subscription {
    pub filter: Option<Vec<String>>,
    pub stream: BroadcastStream<AgentEvent>,
}