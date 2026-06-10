//! 订阅类 handler — 事件流订阅/取消 + confirm 超时清理。
//!
//! ## 为什么不抽取到 service 层？
//! 订阅逻辑强依赖 RunRegistry（在 ws 模块中），
//! 抽取到 service 会产生 service → ws 的循环依赖。
//! 因此保留在 handler 层。
//!
//! ## handler 函数
//! - handle_subscribe     — 订阅活跃 run 的事件流，支持事件类型过滤
//! - handle_unsubscribe   — 取消订阅，移除 subscription
//! - check_confirm_timeouts — 定期清理超时的 pending confirm（由 connection.rs 的 ticker 调用）
//!
//! ## 辅助函数
//! - resolve_run_error — 根据 run 目录是否存在，将通用错误映射为 NotFound 或 RunFinished
//!
//! ## 订阅工作流
//! `text
//!  client                server
//!    │  Subscribe ──────→  │  registry.subscribe(run_id) → Some(rx) → 创建 Subscription
//!    │  ←─ Event ────────  │  poll_subscriptions() 轮询 → 过滤 → 转发
//!    │  ←─ Event ────────  │
//!    │  Unsubscribe ────→  │  移除 Subscription
//! `
use crate::core::contract::ids::RunId;
use crate::ws::protocol::{ErrorCode, ServerMsg};

use super::Subscription;
use super::AppState;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_stream::wrappers::BroadcastStream;

pub async fn handle_subscribe(
    req_id: String,
    payload: crate::ws::protocol::SubscribePayload,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
    subscriptions: &mut HashMap<RunId, Subscription>,
) {
    if let Some(rx) = state.registry.subscribe(&payload.run_id) {
        let sub = Subscription {
            filter: payload.filter.clone(),
            stream: BroadcastStream::new(rx),
        };
        subscriptions.insert(payload.run_id, sub);

        let _ = out_tx.send(ServerMsg::Ok { req_id }).await;
    } else {
        let run_dir = state.base_dir.join(payload.run_id.to_string());
        let code = if run_dir.exists() {
            ErrorCode::RunFinished
        } else {
            ErrorCode::NotFound
        };
        let _ = out_tx
            .send(ServerMsg::Error {
                req_id,
                code,
                message: format!("run {} not active", payload.run_id),
            })
            .await;
    }
}

pub async fn handle_unsubscribe(
    req_id: String,
    run_id: RunId,
    out_tx: &mpsc::Sender<ServerMsg>,
    subscriptions: &mut HashMap<RunId, Subscription>,
) {
    subscriptions.remove(&run_id);
    let _ = out_tx.send(ServerMsg::Ok { req_id }).await;
}

pub async fn check_confirm_timeouts(
    pending_confirms: &mut HashMap<RunId, (String, Instant)>,
    out_tx: &mpsc::Sender<ServerMsg>,
) {
    let now = Instant::now();
    let timeout = Duration::from_secs(30);

    let expired: Vec<RunId> = pending_confirms
        .iter()
        .filter(|(_, (_, ts))| now.duration_since(*ts) > timeout)
        .map(|(id, _)| *id)
        .collect();

    for run_id in expired {
        pending_confirms.remove(&run_id);
        let _ = out_tx
            .send(ServerMsg::Error {
                req_id: format!("confirm-{}", run_id),
                code: ErrorCode::ConfirmTimeout,
                message: format!("confirm_run timeout for run {}", run_id),
            })
            .await;
    }
}

pub fn resolve_run_error(run_id: &RunId, base_dir: &PathBuf, _original_msg: String) -> ErrorCode {
    let run_dir = base_dir.join(run_id.to_string());
    if run_dir.exists() {
        ErrorCode::RunFinished
    } else {
        ErrorCode::NotFound
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::ids::RunId;
    use crate::core::mock_backend::{MockBackend, MockBehavior};
    use crate::ws::handler::AppState;
    use crate::ws::registry::RunRegistry;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc;

    fn test_state(dir: &std::path::Path) -> AppState {
        AppState {
            backend: Arc::new(MockBackend::new("test", vec![MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: Default::default(),
                delay: Duration::ZERO,
            }])),
            registry: RunRegistry::default(),
            base_dir: dir.to_path_buf(),
            run_permits: Arc::new(tokio::sync::Semaphore::new(4)),
            confirm_timeout: Duration::from_secs(30),
        }
    }

    fn make_run_handle() -> crate::ws::registry::RunHandle {
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let cancel = tokio_util::sync::CancellationToken::new();
        let task = tokio::spawn(async {});
        crate::ws::registry::RunHandle {
            events: tx,
            cancel,
            task,
        }
    }

    #[tokio::test]
    async fn check_confirm_timeouts_no_expired() {
        let mut pending = HashMap::new();
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        pending.insert(run_id, ("script".into(), Instant::now()));
        check_confirm_timeouts(&mut pending, &tx).await;
        assert!(pending.contains_key(&run_id));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn check_confirm_timeouts_expired() {
        let mut pending = HashMap::new();
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        pending.insert(run_id, ("script".into(), Instant::now() - Duration::from_secs(60)));
        check_confirm_timeouts(&mut pending, &tx).await;
        assert!(!pending.contains_key(&run_id));
        let msg = rx.try_recv().unwrap();
        match msg {
            crate::ws::protocol::ServerMsg::Error { code, .. } => {
                assert!(matches!(code, crate::ws::protocol::ErrorCode::ConfirmTimeout));
            }
            _ => panic!("expected error"),
        }
    }

    #[test]
    fn resolve_run_error_dir_exists() {
        let dir = std::env::temp_dir().join("maestro_test_resolve_exists");
        std::fs::create_dir_all(&dir).unwrap();
        let run_id = RunId::now_v7();
        let run_dir = dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();
        let code = resolve_run_error(&run_id, &dir, "test".into());
        assert!(matches!(code, ErrorCode::RunFinished));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_run_error_dir_missing() {
        let dir = std::env::temp_dir().join("maestro_test_resolve_missing");
        let run_id = RunId::now_v7();
        let code = resolve_run_error(&run_id, &dir, "test".into());
        assert!(matches!(code, ErrorCode::NotFound));
    }

    #[tokio::test]
    async fn handle_unsubscribe_removes_and_responds() {
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        let mut subs = HashMap::new();
        subs.insert(run_id, super::Subscription {
            filter: None,
            stream: tokio_stream::wrappers::BroadcastStream::new(
                tokio::sync::broadcast::channel(16).1
            ),
        });
        handle_unsubscribe("req1".into(), run_id, &tx, &mut subs).await;
        assert!(!subs.contains_key(&run_id));
        let msg = rx.try_recv().unwrap();
        assert!(matches!(msg, crate::ws::protocol::ServerMsg::Ok { ref req_id } if req_id == "req1"));
    }

    #[tokio::test]
    async fn handle_subscribe_active_run() {
        let dir = std::env::temp_dir().join("maestro_test_sub_active");
        let state = test_state(&dir);
        let run_id = RunId::now_v7();
        state.registry.insert(run_id, make_run_handle());
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        handle_subscribe(
            "req2".into(),
            crate::ws::protocol::SubscribePayload { run_id, filter: Some(vec!["run_started".into()]) },
            &state,
            &tx,
            &mut subs,
        ).await;
        assert!(subs.contains_key(&run_id));
        let msg = rx.try_recv().unwrap();
        assert!(matches!(msg, crate::ws::protocol::ServerMsg::Ok { .. }));
    }

    #[tokio::test]
    async fn handle_subscribe_inactive_run_not_found() {
        let dir = std::env::temp_dir().join("maestro_test_sub_notfound");
        let state = test_state(&dir);
        let run_id = RunId::now_v7();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        handle_subscribe(
            "req3".into(),
            crate::ws::protocol::SubscribePayload { run_id, filter: None },
            &state,
            &tx,
            &mut subs,
        ).await;
        assert!(!subs.contains_key(&run_id));
        let msg = rx.try_recv().unwrap();
        match msg {
            crate::ws::protocol::ServerMsg::Error { code, .. } => {
                assert!(matches!(code, ErrorCode::NotFound));
            }
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn handle_subscribe_inactive_run_finished() {
        let dir = std::env::temp_dir().join("maestro_test_sub_finished");
        std::fs::create_dir_all(&dir).unwrap();
        let state = test_state(&dir);
        let run_id = RunId::now_v7();
        let run_dir = dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        handle_subscribe(
            "req4".into(),
            crate::ws::protocol::SubscribePayload { run_id, filter: None },
            &state,
            &tx,
            &mut subs,
        ).await;
        assert!(!subs.contains_key(&run_id));
        let msg = rx.try_recv().unwrap();
        match msg {
            crate::ws::protocol::ServerMsg::Error { code, .. } => {
                assert!(matches!(code, ErrorCode::RunFinished));
            }
            _ => panic!("expected error"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
