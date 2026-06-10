//! 运行类 handler — run 生命周期管理（创建/确认/恢复/取消）。
//!
//! ## 架构定位
//! handler 层只做**协议适配**和**资源管理**:
//! - 协议适配: 解析 payload → 调 service → 映射结果为 ServerMsg
//! - 资源管理: semaphore permit 获取/释放、RunRegistry 注册/移除、后台 task spawn
//!
//! 业务逻辑委托给 crate::service::run:
//! - [service::run::validate_source] — 验证 nl/workflow/script 三选一
//! - [service::run::resolve_script]  — NL→规划 / 读文件 / 直传脚本
//! - [service::run::check_resumable] — 检查 checkpoint 状态是否可恢复
//!
//! ## handler 函数一览
//!
//! | 函数                  | 触发消息       | 职责                                    |
//! |-----------------------|---------------|-----------------------------------------|
//! | handle_run          | ClientMsg::Run        | 验证 → 获取 permit → 解析脚本 → spawn 后台 run |
//! | handle_confirm_run  | ClientMsg::ConfirmRun | 用户确认/拒绝 pending 的 NL 规划结果        |
//! | handle_resume       | ClientMsg::Resume     | 检查可恢复性 → spawn 后台 resume run        |
//! | handle_cancel       | ClientMsg::Cancel     | 取消活跃 run（通过 CancellationToken）       |
//!
//! ## 并发控制
//! - state.run_permits (Semaphore) 限制同时运行的 run 数量
//! - 每个活跃 run 注册到 state.registry (RunRegistry / DashMap)
//! - run 完成后自动从 registry 移除并释放 permit
//!
//! ## confirm 流程
//! 1. 客户端发送 { type: "run", confirm: true, nl: "..." }
//! 2. handler 调 planner 生成 script → 存入 pending_confirms → 返回 ScriptPreview
//! 3. 客户端确认 { type: "confirm_run", approve: true }
//! 4. handler 从 pending_confirms 取出 script → spawn 执行
//! 5. 超时未确认的条目由 sub::check_confirm_timeouts 定期清理
use crate::cli;
use crate::core::contract::ids::RunId;
use crate::ws::protocol::{ErrorCode, ServerMsg};
use crate::ws::registry::RunHandle;

use super::Subscription;
use super::AppState;

use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::mpsc;

#[allow(clippy::too_many_arguments)]
pub async fn handle_run(
    req_id: String,
    payload: crate::ws::protocol::RunPayload,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
    _subscriptions: &mut HashMap<RunId, Subscription>,
    pending_confirms: &mut HashMap<RunId, (String, Instant)>,
) {
    use crate::service::run::{validate_source, RunInput, ValidateSourceError};
    let input = RunInput { nl: payload.nl.clone(), workflow: payload.workflow.clone(), script: payload.script.clone() };
    if let Err(e) = validate_source(&input) {
        let msg = match e {
            ValidateSourceError::NoneProvided => "exactly one of nl, workflow, or script must be provided",
            ValidateSourceError::MultipleProvided => "exactly one of nl, workflow, or script must be provided",
        };
        let _ = out_tx
            .send(ServerMsg::Error { req_id, code: ErrorCode::BadRequest, message: msg.to_string() })
            .await;
        return;
    }

    let permit = match state.run_permits.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            let _ = out_tx
                .send(ServerMsg::Error {
                    req_id,
                    code: ErrorCode::Capacity,
                    message: "max concurrent runs reached".to_string(),
                })
                .await;
            return;
        }
    };

    let run_id = uuid::Uuid::now_v7();
    let (events_tx, _events_rx) = tokio::sync::broadcast::channel(256);
    let cancel = tokio_util::sync::CancellationToken::new();

    if payload.confirm && payload.nl.is_some() {
        let nl = payload.nl.as_ref().unwrap();
        match crate::service::run::resolve_script(
            crate::service::run::ScriptSource::Nl(nl),
            state.backend.clone(),
        ).await {
            Ok(script) => {
                pending_confirms.insert(run_id, (script.clone(), Instant::now()));
                let _ = out_tx
                    .send(ServerMsg::ScriptPreview { req_id, run_id, script })
                    .await;
            }
            Err(e) => {
                let _ = out_tx
                    .send(ServerMsg::Error { req_id, code: ErrorCode::BackendError, message: format!("planning failed: {}", e) })
                    .await;
            }
        }
        drop(permit);
        return;
    }

    let backend = state.backend.clone();
    let registry = state.registry.clone();

    let script_result = if let Some(ref nl) = payload.nl {
        crate::service::run::resolve_script(
            crate::service::run::ScriptSource::Nl(nl),
            backend.clone(),
        ).await
    } else if let Some(ref path) = payload.workflow {
        crate::service::run::resolve_script(
            crate::service::run::ScriptSource::Workflow(path),
            backend.clone(),
        ).await
    } else {
        Ok(payload.script.unwrap_or_default())
    };
    let script = match script_result {
        Ok(s) => s,
        Err(e) => {
            drop(permit);
            let code = if let Some(ref nl) = payload.nl {
                ErrorCode::BackendError
            } else {
                ErrorCode::BadRequest
            };
            let _ = out_tx
                .send(ServerMsg::Error { req_id, code, message: e.to_string() })
                .await;
            return;
        }
    };

    let events_tx_clone = events_tx.clone();
    let cancel_clone = cancel.clone();
    let run_args = cli::RunArgs {
        nl: payload.nl,
        workflow: payload.workflow,
        script: Some(script),
        resume: false,
        mode: cli::RunMode::Headless,
        approve: true,
        extra_args: payload.args,
        output: None,
        events_tx: Some(events_tx),
    };

    let task = tokio::spawn(async move {
        let _ = cli::run(backend, run_args).await;
        registry.remove(&run_id);
        drop(permit);
    });

    state.registry.insert(
        run_id,
        RunHandle {
            events: events_tx_clone,
            cancel: cancel_clone,
            task,
        },
    );

    let _ = out_tx
        .send(ServerMsg::Accepted { req_id, run_id })
        .await;
}

pub async fn handle_confirm_run(
    req_id: String,
    payload: crate::ws::protocol::ConfirmRunPayload,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
    pending_confirms: &mut HashMap<RunId, (String, Instant)>,
) {
    if let Some((_script, _ts)) = pending_confirms.remove(&payload.run_id) {
        if payload.approve {
            let permit = match state.run_permits.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    let _ = out_tx
                        .send(ServerMsg::Error {
                            req_id,
                            code: ErrorCode::Capacity,
                            message: "max concurrent runs reached".to_string(),
                        })
                        .await;
                    return;
                }
            };

            let (events_tx, _events_rx) = tokio::sync::broadcast::channel(256);
            let cancel = tokio_util::sync::CancellationToken::new();
            let run_id = payload.run_id;
            let backend = state.backend.clone();
            let registry = state.registry.clone();

            let events_tx_clone = events_tx.clone();
            let cancel_clone = cancel.clone();
            let run_args = cli::RunArgs {
                nl: None,
                workflow: None,
                script: Some(_script),
                resume: false,
                mode: cli::RunMode::Headless,
                approve: true,
                extra_args: serde_json::json!({}),
                output: None,
                events_tx: Some(events_tx),
            };

            let task = tokio::spawn(async move {
                let _ = cli::run(backend, run_args).await;
                registry.remove(&run_id);
                drop(permit);
            });

            state.registry.insert(
                run_id,
                RunHandle {
                    events: events_tx_clone,
                    cancel: cancel_clone,
                    task,
                },
            );

            let _ = out_tx.send(ServerMsg::Ok { req_id }).await;
        } else {
            let _ = out_tx.send(ServerMsg::Ok { req_id }).await;
        }
    } else {
        let _ = out_tx
            .send(ServerMsg::Error {
                req_id,
                code: ErrorCode::NotFound,
                message: format!(
                    "no pending confirm for run {} (expired or never created)",
                    payload.run_id
                ),
            })
            .await;
    }
}

pub async fn handle_resume(
    req_id: String,
    run_id: RunId,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
) {
    if state.registry.contains(&run_id) {
        let _ = out_tx
            .send(ServerMsg::Error { req_id, code: ErrorCode::AlreadyRunning, message: format!("run {} is already running", run_id) })
            .await;
        return;
    }

    match crate::service::run::check_resumable(run_id, &state.base_dir) {
        crate::service::run::ResumeCheck::NotFound => {
            let _ = out_tx
                .send(ServerMsg::Error { req_id, code: ErrorCode::NotFound, message: format!("run {} not found", run_id) })
                .await;
            return;
        }
        crate::service::run::ResumeCheck::NotResumable(status) => {
            let _ = out_tx
                .send(ServerMsg::Error { req_id, code: ErrorCode::RunFinished, message: format!("run {} is not resumable (status: {:?})", run_id, status) })
                .await;
            return;
        }
        crate::service::run::ResumeCheck::CanResume => {}
    }

    let permit = match state.run_permits.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            let _ = out_tx
                .send(ServerMsg::Error {
                    req_id,
                    code: ErrorCode::Capacity,
                    message: "max concurrent runs reached".to_string(),
                })
                .await;
            return;
        }
    };

    let (events_tx, _events_rx) = tokio::sync::broadcast::channel(256);
    let cancel = tokio_util::sync::CancellationToken::new();
    let backend = state.backend.clone();
    let registry = state.registry.clone();

    let events_tx_clone = events_tx.clone();
    let cancel_clone = cancel.clone();
    let run_args = cli::RunArgs {
        nl: None,
        workflow: None,
        script: None,
        resume: true,
        mode: cli::RunMode::Headless,
        approve: true,
        extra_args: serde_json::json!({}),
        output: None,
        events_tx: Some(events_tx),
    };

    let task = tokio::spawn(async move {
        let _ = cli::run(backend, run_args).await;
        registry.remove(&run_id);
        drop(permit);
    });

    state.registry.insert(
        run_id,
        RunHandle {
            events: events_tx_clone,
            cancel: cancel_clone,
            task,
        },
    );

    let _ = out_tx
        .send(ServerMsg::Accepted { req_id, run_id })
        .await;
}

pub async fn handle_cancel(
    req_id: String,
    run_id: RunId,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
) {
    if state.registry.cancel(&run_id) {
        let _ = out_tx.send(ServerMsg::Ok { req_id }).await;
    } else {
        let run_dir = state.base_dir.join(run_id.to_string());
        let code = if run_dir.exists() {
            ErrorCode::RunFinished
        } else {
            ErrorCode::NotFound
        };
        let _ = out_tx
            .send(ServerMsg::Error {
                req_id,
                code,
                message: format!("run {} not found", run_id),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mock_backend::{MockBackend, MockBehavior};
    use crate::ws::handler::AppState;
    use crate::ws::registry::RunRegistry;
    use std::sync::Arc;
    use std::time::Duration;
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
    async fn cancel_active_run() {
        let dir = std::env::temp_dir().join("maestro_test_run_cancel");
        let state = test_state(&dir);
        let run_id = RunId::now_v7();
        state.registry.insert(run_id, make_run_handle());
        let (tx, mut rx) = mpsc::channel(16);
        handle_cancel("c1".into(), run_id, &state, &tx).await;
        assert!(matches!(rx.try_recv().unwrap(), ServerMsg::Ok { .. }));
    }

    #[tokio::test]
    async fn cancel_missing_run() {
        let dir = std::env::temp_dir().join("maestro_test_run_cancel2");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        handle_cancel("c2".into(), RunId::now_v7(), &state, &tx).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::NotFound)),
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn cancel_finished_run() {
        let dir = std::env::temp_dir().join("maestro_test_run_cancel3");
        let state = test_state(&dir);
        let run_id = RunId::now_v7();
        std::fs::create_dir_all(dir.join(run_id.to_string())).unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        handle_cancel("c3".into(), run_id, &state, &tx).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::RunFinished)),
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn resume_missing_run() {
        let dir = std::env::temp_dir().join("maestro_test_run_resume");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        handle_resume("r1".into(), RunId::now_v7(), &state, &tx).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::NotFound)),
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn confirm_run_no_pending() {
        let dir = std::env::temp_dir().join("maestro_test_run_confirm");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        let mut pending = HashMap::new();
        handle_confirm_run(
            "cr1".into(),
            crate::ws::protocol::ConfirmRunPayload { run_id, approve: true },
            &state, &tx, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::NotFound)),
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn run_no_source_returns_bad_request() {
        let dir = std::env::temp_dir().join("maestro_test_run_noop");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        handle_run(
            "r1".into(),
            crate::ws::protocol::RunPayload {
                nl: None, workflow: None, script: None,
                args: serde_json::Value::Null, confirm: false,
            },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::BadRequest)),
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn run_multiple_sources_returns_bad_request() {
        let dir = std::env::temp_dir().join("maestro_test_run_multi");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        handle_run(
            "r2".into(),
            crate::ws::protocol::RunPayload {
                nl: Some("hi".into()), script: Some("print(1)".into()),
                workflow: None, args: serde_json::Value::Null, confirm: false,
            },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::BadRequest)),
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn run_with_capacity_error() {
        let dir = std::env::temp_dir().join("maestro_test_run_capacity");
        let state = AppState {
            backend: test_state(&dir).backend,
            registry: test_state(&dir).registry,
            base_dir: test_state(&dir).base_dir,
            run_permits: Arc::new(tokio::sync::Semaphore::new(0)),
            confirm_timeout: test_state(&dir).confirm_timeout,
        };
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        handle_run(
            "r3".into(),
            crate::ws::protocol::RunPayload {
                nl: Some("hello".into()), script: None, workflow: None,
                args: serde_json::Value::Null, confirm: false,
            },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::Capacity)),
            _ => panic!("expected capacity error"),
        }
    }

    #[tokio::test]
    async fn run_with_nl_confirm_planning_success() {
        let dir = std::env::temp_dir().join("maestro_test_run_confirm_ok");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        handle_run(
            "r4".into(),
            crate::ws::protocol::RunPayload {
                nl: Some("test".into()), script: None, workflow: None,
                args: serde_json::Value::Null, confirm: true,
            },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::ScriptPreview { req_id: _, run_id: _, script } => {
                assert!(!script.is_empty());
            }
            _ => panic!("expected script preview"),
        }
    }

    #[tokio::test]
    async fn run_with_nl_confirm_planning_failure() {
        let dir = std::env::temp_dir().join("maestro_test_run_confirm_fail");
        let error_backend = Arc::new(MockBackend::new("test", vec![MockBehavior::Fail {
            kind: crate::core::contract::FailKind::Protocol,
            delay: Duration::ZERO,
        }]));
        let state = AppState {
            backend: error_backend,
            registry: test_state(&dir).registry,
            base_dir: test_state(&dir).base_dir,
            run_permits: test_state(&dir).run_permits,
            confirm_timeout: test_state(&dir).confirm_timeout,
        };
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        handle_run(
            "r5".into(),
            crate::ws::protocol::RunPayload {
                nl: Some("bad".into()), script: None, workflow: None,
                args: serde_json::Value::Null, confirm: true,
            },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::BackendError)),
            _ => panic!("expected backend error"),
        }
    }

    #[tokio::test]
    async fn run_with_nl_planning_failure() {
        let dir = std::env::temp_dir().join("maestro_test_run_nl_fail");
        let error_backend = Arc::new(MockBackend::new("test", vec![MockBehavior::Fail {
            kind: crate::core::contract::FailKind::Protocol,
            delay: Duration::ZERO,
        }]));
        let state = AppState {
            backend: error_backend,
            registry: test_state(&dir).registry,
            base_dir: test_state(&dir).base_dir,
            run_permits: test_state(&dir).run_permits,
            confirm_timeout: test_state(&dir).confirm_timeout,
        };
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        handle_run(
            "r6".into(),
            crate::ws::protocol::RunPayload {
                nl: Some("bad".into()), script: None, workflow: None,
                args: serde_json::Value::Null, confirm: false,
            },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::BackendError)),
            _ => panic!("expected backend error"),
        }
    }

    #[tokio::test]
    async fn run_with_workflow_file_not_found() {
        let dir = std::env::temp_dir().join("maestro_test_run_workflow");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let mut subs = HashMap::new();
        let mut pending = HashMap::new();
        handle_run(
            "r7".into(),
            crate::ws::protocol::RunPayload {
                nl: None, script: None, workflow: Some("/nonexistent/workflow.lua".into()),
                args: serde_json::Value::Null, confirm: false,
            },
            &state, &tx, &mut subs, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::BadRequest)),
            _ => panic!("expected bad request"),
        }
    }

    #[tokio::test]
    async fn confirm_run_approve_success() {
        let dir = std::env::temp_dir().join("maestro_test_confirm_approve");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        let mut pending = HashMap::new();
        pending.insert(run_id, ("print('approved')".to_string(), std::time::Instant::now()));
        handle_confirm_run(
            "ca1".into(),
            crate::ws::protocol::ConfirmRunPayload { run_id, approve: true },
            &state, &tx, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Ok { req_id } => assert_eq!(req_id, "ca1"),
            _ => panic!("expected ok"),
        }
    }

    #[tokio::test]
    async fn confirm_run_approve_capacity_error() {
        let dir = std::env::temp_dir().join("maestro_test_confirm_capacity");
        let state = AppState {
            backend: test_state(&dir).backend,
            registry: test_state(&dir).registry,
            base_dir: test_state(&dir).base_dir,
            run_permits: Arc::new(tokio::sync::Semaphore::new(0)),
            confirm_timeout: test_state(&dir).confirm_timeout,
        };
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        let mut pending = HashMap::new();
        pending.insert(run_id, ("print('full')".to_string(), std::time::Instant::now()));
        handle_confirm_run(
            "ca2".into(),
            crate::ws::protocol::ConfirmRunPayload { run_id, approve: true },
            &state, &tx, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::Capacity)),
            _ => panic!("expected capacity error"),
        }
    }

    #[tokio::test]
    async fn confirm_run_reject_success() {
        let dir = std::env::temp_dir().join("maestro_test_confirm_reject");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        let mut pending = HashMap::new();
        pending.insert(run_id, ("print('rejected')".to_string(), std::time::Instant::now()));
        handle_confirm_run(
            "cr1".into(),
            crate::ws::protocol::ConfirmRunPayload { run_id, approve: false },
            &state, &tx, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Ok { req_id } => assert_eq!(req_id, "cr1"),
            _ => panic!("expected ok"),
        }
        assert!(!pending.contains_key(&run_id));
    }

    #[tokio::test]
    async fn resume_already_running() {
        let dir = std::env::temp_dir().join("maestro_test_resume_running");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        state.registry.insert(run_id, make_run_handle());
        handle_resume("res1".into(), run_id, &state, &tx).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::AlreadyRunning)),
            _ => panic!("expected already running error"),
        }
    }

    #[tokio::test]
    async fn resume_with_capacity_error() {
        let dir = std::env::temp_dir().join("maestro_test_resume_capacity");
        let state = AppState {
            backend: test_state(&dir).backend,
            registry: test_state(&dir).registry,
            base_dir: test_state(&dir).base_dir,
            run_permits: Arc::new(tokio::sync::Semaphore::new(0)),
            confirm_timeout: test_state(&dir).confirm_timeout,
        };
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        let run_dir = dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();
        handle_resume("res2".into(), run_id, &state, &tx).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::Capacity)),
            _ => panic!("expected capacity error"),
        }
    }

    #[tokio::test]
    async fn resume_completed_run() {
        let dir = std::env::temp_dir().join("maestro_test_resume_completed");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        let run_dir = dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();
        
        let checkpoint = serde_json::json!({
            "status": "completed"
        });
        std::fs::write(run_dir.join("checkpoint.json"), serde_json::to_string(&checkpoint).unwrap()).unwrap();
        
        handle_resume("res3".into(), run_id, &state, &tx).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::RunFinished)),
            _ => panic!("expected run finished error"),
        }
    }

    #[tokio::test]
    async fn resume_cancelled_run() {
        let dir = std::env::temp_dir().join("maestro_test_resume_cancelled");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        let run_dir = dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();
        
        let checkpoint = serde_json::json!({
            "status": "cancelled"
        });
        std::fs::write(run_dir.join("checkpoint.json"), serde_json::to_string(&checkpoint).unwrap()).unwrap();
        
        handle_resume("res4".into(), run_id, &state, &tx).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::RunFinished)),
            _ => panic!("expected run finished error"),
        }
    }

    #[tokio::test]
    async fn resume_failed_run() {
        let dir = std::env::temp_dir().join("maestro_test_resume_failed");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        let run_dir = dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();
        
        let checkpoint = serde_json::json!({
            "status": "failed"
        });
        std::fs::write(run_dir.join("checkpoint.json"), serde_json::to_string(&checkpoint).unwrap()).unwrap();
        
        handle_resume("res5".into(), run_id, &state, &tx).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::RunFinished)),
            _ => panic!("expected run finished error"),
        }
    }

    #[tokio::test]
    async fn resume_resumable_run_success() {
        let dir = std::env::temp_dir().join("maestro_test_resume_success");
        let state = test_state(&dir);
        let (tx, mut rx) = mpsc::channel(16);
        let run_id = RunId::now_v7();
        let run_dir = dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();
        
        let checkpoint = serde_json::json!({
            "status": "running"
        });
        std::fs::write(run_dir.join("checkpoint.json"), serde_json::to_string(&checkpoint).unwrap()).unwrap();
        
        handle_resume("res6".into(), run_id, &state, &tx).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Accepted { req_id, run_id: id } => {
                assert_eq!(req_id, "res6");
                assert_eq!(id, run_id);
            }
            _ => panic!("expected accepted"),
        }
    }
}
