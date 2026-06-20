//! 消息分发器 — ClientMsg 路由到对应 handler 函数。
//!
//! 这是 handler 层的中央路由。connection.rs 将解析后的 ClientMsg 传入
//! [dispatch_client_msg]，该函数通过 match 将消息分发到具体的 handler:
//!
//! | ClientMsg 变体    | 目标 handler           | 分类   |
//! |-------------------|------------------------|--------|
//! | Ping            | 内联（直接回 Pong）     | 心跳   |
//! | GetStatus       | query::handle_get_status  | 查询   |
//! | ListRuns        | query::handle_list_runs   | 查询   |
//! | GetLogs         | query::handle_get_logs    | 查询   |
//! | GetFindings     | query::handle_get_findings| 查询   |
//! | GetReport       | query::handle_get_report  | 查询   |
//! | Run             | run::handle_run           | 运行   |
//! | ConfirmRun      | run::handle_confirm_run   | 运行   |
//! | Resume          | run::handle_resume        | 运行   |
//! | Cancel          | run::handle_cancel        | 运行   |
//! | Subscribe       | sub::handle_subscribe     | 订阅   |
//! | Unsubscribe     | sub::handle_unsubscribe   | 订阅   |
//!
//! 所有 handler 共享相同的签名模式:
//! (req_id, payload, state, out_tx, ...) → ()
//! - req_id 用于响应关联
//! - out_tx 用于异步发送 ServerMsg 回客户端
//! - handler 不返回值，错误通过 ServerMsg::Error 发送
use crate::core::contract::ids::RunId;
use crate::service::run::RunSpec;
use crate::ws::protocol::{ClientMsg, ServerMsg};

use super::sub;
use super::query;
use super::run;
use super::Subscription;
use super::AppState;

use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::mpsc;

pub async fn dispatch_client_msg(
    msg: ClientMsg,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
    subscriptions: &mut HashMap<RunId, Subscription>,
    pending_confirms: &mut HashMap<RunId, (RunSpec, Instant)>,
) {
    match msg {
        ClientMsg::Ping { id } => {
            let _ = out_tx.send(ServerMsg::Pong { req_id: id }).await;
        }

        ClientMsg::GetStatus { id, payload } => {
            query::handle_get_status(id, payload.run_id, state, out_tx).await;
        }

        ClientMsg::ListRuns { id, payload } => {
            query::handle_list_runs(id, payload.limit, payload.offset, state, out_tx).await;
        }

        ClientMsg::GetLogs { id, payload } => {
            query::handle_get_logs(id, payload.run_id, payload.limit, payload.offset, state, out_tx)
                .await;
        }

        ClientMsg::GetFindings { id, payload } => {
            query::handle_get_findings(id, payload.run_id, state, out_tx).await;
        }

        ClientMsg::GetReport { id, payload } => {
            query::handle_get_report(id, payload.run_id, state, out_tx).await;
        }

        ClientMsg::Run { id, payload } => {
            run::handle_run(id, payload, state, out_tx, pending_confirms)
                .await;
        }

        ClientMsg::ConfirmRun { id, payload } => {
            run::handle_confirm_run(id, payload, state, out_tx, pending_confirms)
                .await;
        }

        ClientMsg::Resume { id, payload } => {
            run::handle_resume(id, payload.run_id, state, out_tx).await;
        }

        ClientMsg::Cancel { id, payload } => {
            run::handle_cancel(id, payload.run_id, state, out_tx).await;
        }

        ClientMsg::Subscribe { id, payload } => {
            sub::handle_subscribe(id, payload, state, out_tx, subscriptions).await;
        }

        ClientMsg::Unsubscribe { id, payload } => {
            sub::handle_unsubscribe(id, payload.run_id, out_tx, subscriptions).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mock_backend::{MockBackend, MockBehavior};
    use crate::ws::handler::AppState;
    use crate::ws::registry::{RunRegistry, RunHandle};
    use std::sync::Arc;
    use std::time::Duration;

    fn test_state() -> AppState {
        AppState {
            backend: Arc::new(MockBackend::new("test", vec![MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: Default::default(),
                delay: Duration::ZERO,
            }])),
            registry: RunRegistry::default(),
            base_dir: std::env::temp_dir().join("maestro_test_dispatch"),
            run_permits: Arc::new(tokio::sync::Semaphore::new(4)),
            confirm_timeout: Duration::from_secs(30),
        }
    }

    fn make_run_handle() -> RunHandle {
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let cancel = tokio_util::sync::CancellationToken::new();
        let task = tokio::spawn(async {});
        RunHandle {
            events: tx,
            cancel,
            task,
        }
    }

    #[tokio::test]
    async fn dispatch_ping_returns_pong() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        dispatch_client_msg(ClientMsg::Ping { id: "p1".into() }, &state, &tx, &mut subs, &mut pending).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Pong { req_id } => assert_eq!(req_id, "p1"),
            _ => panic!("expected pong"),
        }
    }

    #[tokio::test]
    async fn dispatch_unsubscribe_removes_subscription() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        let mut subs = HashMap::new();
        subs.insert(run_id, super::Subscription {
            filter: None,
            stream: tokio_stream::wrappers::BroadcastStream::new(
                tokio::sync::broadcast::channel(16).1
            ),
        });
        let mut pending = HashMap::new();
        dispatch_client_msg(
            ClientMsg::Unsubscribe { id: "u1".into(), payload: crate::ws::protocol::IdPayload { run_id } },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        assert!(!subs.contains_key(&run_id));
        assert!(matches!(rx.try_recv().unwrap(), ServerMsg::Ok { .. }));
    }

    #[tokio::test]
    async fn dispatch_get_status_not_found() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        dispatch_client_msg(
            ClientMsg::GetStatus { id: "gs1".into(), payload: crate::ws::protocol::IdPayload { run_id: RunId::now_v7() } },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, crate::ws::protocol::ErrorCode::NotFound)),
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn dispatch_cancel_not_found() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        dispatch_client_msg(
            ClientMsg::Cancel { id: "c1".into(), payload: crate::ws::protocol::IdPayload { run_id: RunId::now_v7() } },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { .. } => {}
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn dispatch_run_no_payload_returns_error() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        dispatch_client_msg(
            ClientMsg::Run { id: "r1".into(), payload: crate::ws::protocol::RunPayload {
                nl: None, workflow: None, script: None, args: serde_json::Value::Null, confirm: false,
            }},
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, crate::ws::protocol::ErrorCode::BadRequest)),
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn dispatch_list_runs_empty() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        dispatch_client_msg(
            ClientMsg::ListRuns { id: "lr1".into(), payload: crate::ws::protocol::ListRunsPayload { limit: 20, offset: 0 } },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::RunList { total, items, .. } => {
                assert_eq!(total, 0);
                assert!(items.is_empty());
            }
            _ => panic!("expected run list"),
        }
    }

    #[tokio::test]
    async fn dispatch_get_logs_not_found() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        dispatch_client_msg(
            ClientMsg::GetLogs { id: "gl1".into(), payload: crate::ws::protocol::GetLogsPayload { run_id: RunId::now_v7(), limit: 20, offset: 0 } },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { .. } => {}
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn dispatch_get_findings_empty() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        dispatch_client_msg(
            ClientMsg::GetFindings { id: "gf1".into(), payload: crate::ws::protocol::IdPayload { run_id: RunId::now_v7() } },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Findings { items, .. } => assert!(items.is_empty()),
            ServerMsg::Error { .. } => {}
            other => panic!("expected findings or error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn dispatch_get_report_not_found() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        dispatch_client_msg(
            ClientMsg::GetReport { id: "gr1".into(), payload: crate::ws::protocol::IdPayload { run_id: RunId::now_v7() } },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, crate::ws::protocol::ErrorCode::NotFound)),
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn dispatch_confirm_run_no_pending() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        dispatch_client_msg(
            ClientMsg::ConfirmRun { id: "cr1".into(), payload: crate::ws::protocol::ConfirmRunPayload { run_id: RunId::now_v7(), approve: true } },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, crate::ws::protocol::ErrorCode::NotFound)),
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn dispatch_resume_not_found() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        dispatch_client_msg(
            ClientMsg::Resume { id: "res1".into(), payload: crate::ws::protocol::IdPayload { run_id: RunId::now_v7() } },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, crate::ws::protocol::ErrorCode::NotFound)),
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn dispatch_subscribe_with_filter() {
        let state = test_state();
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        state.registry.insert(run_id, make_run_handle());
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        dispatch_client_msg(
            ClientMsg::Subscribe { id: "sub1".into(), payload: crate::ws::protocol::SubscribePayload { run_id, filter: Some(vec!["run_started".to_string()]) } },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Ok { .. } => assert!(subs.contains_key(&run_id)),
            _ => panic!("expected ok"),
        }
    }
}
