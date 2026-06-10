//! WebSocket 连接生命周期管理。
//!
//! 负责 WS 连接建立后的整个交互循环:
//!
//! 1. 发送 Hello 握手消息（含版本号和 capabilities）
//! 2. 启动写任务（write_task），从 out_tx channel 接收 ServerMsg 并序列化为 JSON 发送
//! 3. 主循环通过 	okio::select! 同时处理:
//!    - 客户端消息（stream.next()）→ 反序列化为 ClientMsg → 调用 dispatch::dispatch_client_msg
//!    - 订阅事件（poll_subscriptions）→ 转发给客户端
//!    - confirm 超时检查（check_confirm_timeouts）
//!    - 定时心跳 ticker
//!
//! 连接断开时自动清理：abort 写任务、释放 semaphore permit。
use crate::ws::protocol::{default_capabilities, ClientMsg, ErrorCode, ServerMsg};

use super::Subscription;
use super::AppState;

use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub(super) async fn handle_ws(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();

    let (out_tx, mut out_rx) = mpsc::channel::<ServerMsg>(64);

    let hello = ServerMsg::Hello {
        version: "0.1.0",
        server: "maestro",
        capabilities: default_capabilities(),
    };
    if out_tx.send(hello).await.is_err() {
        return;
    }

    let write_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    tracing::error!("failed to serialize server message: {}", e);
                    continue;
                }
            };
            if sink.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    let mut subscriptions: HashMap<_, Subscription> = HashMap::new();
    let mut pending_confirms: HashMap<_, (String, Instant)> = HashMap::new();

    let mut timeout_ticker = tokio::time::interval(Duration::from_secs(5));

    loop {
        tokio::select! {
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let client_msg = match serde_json::from_str::<ClientMsg>(&text) {
                            Ok(m) => m,
                            Err(e) => {
                                tracing::debug!("invalid client message: {}", e);
                                let _ = out_tx.send(ServerMsg::Error {
                                    req_id: "unknown".to_string(),
                                    code: ErrorCode::BadRequest,
                                    message: format!("invalid JSON: {}", e),
                                }).await;
                                continue;
                            }
                        };
                        super::dispatch::dispatch_client_msg(
                            client_msg,
                            &state,
                            &out_tx,
                            &mut subscriptions,
                            &mut pending_confirms,
                        ).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }
                    Some(Ok(Message::Ping(_))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::debug!("websocket receive error: {}", e);
                        break;
                    }
                }
            }

            Some((run_id, evt)) = super::subscription::poll_subscriptions(&mut subscriptions) => {
                let msg = ServerMsg::Event { run_id, event: evt };
                if out_tx.send(msg).await.is_err() {
                    break;
                }
            }

            _ = timeout_ticker.tick() => {
                super::sub::check_confirm_timeouts(&mut pending_confirms, &out_tx).await;
            }
        }
    }

    write_task.abort();
    drop(out_tx);
}
