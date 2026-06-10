//! Axum 路由入口 — HTTP/WS 端点定义。
//!
//! 提供两个端点:
//! - `GET /ws`      → WebSocket 升级，委托给 `connection::handle_ws` 处理整个连接生命周期
//! - `GET /health`  → 健康检查，返回 `{"ok": true, "version": "0.1.0"}`

use crate::ws::handler::connection;
use crate::ws::handler::AppState;

use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;

/// WebSocket 升级 handler。
///
/// 限制最大帧大小为 64 KB（与协议文档一致），升级成功后进入
/// `connection::handle_ws` 的长连接循环。
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.max_frame_size(64 * 1024)
        .on_upgrade(move |socket| connection::handle_ws(socket, state))
}

/// 健康检查端点，供负载均衡器 / 监控系统探测。
pub async fn health_handler() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "ok": true,
        "version": "0.1.0"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_returns_ok() {
        let axum::Json(json) = health_handler().await;
        assert_eq!(json["ok"], true);
        assert_eq!(json["version"], "0.1.0");
    }
}