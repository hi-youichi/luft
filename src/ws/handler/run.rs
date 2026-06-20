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
use crate::core::contract::backend::RunContext;
use crate::core::contract::event::AgentEvent;
use crate::core::contract::ids::RunId;
use crate::service::run::RunSpec;
use crate::ws::protocol::{ErrorCode, ServerMsg};
use crate::ws::registry::RunHandle;

use super::AppState;

use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::{broadcast, mpsc, OwnedSemaphorePermit};
use tokio_util::sync::CancellationToken;

/// Build a [`RunContext`] from caller-owned event/cancel handles, prepare the
/// run via the shared service, spawn its execution, and register it in the
/// run registry. The `events_tx` and `cancel` are stored in the [`RunHandle`]
/// so subscribers can stream events and `cancel` can stop the run.
///
/// On preparation failure the `permit` is released (dropped here) and the error
/// is returned for the caller to map onto a `ServerMsg::Error`.
fn spawn_prepared(
    state: &AppState,
    spec: RunSpec,
    events_tx: broadcast::Sender<AgentEvent>,
    cancel: CancellationToken,
    permit: OwnedSemaphorePermit,
) -> anyhow::Result<()> {
    let run_id = spec.run_id;
    let run_ctx = RunContext {
        run_id,
        cancel: cancel.clone(),
        events: events_tx.clone(),
    };
    let prepared =
        crate::service::run::prepare(&spec, state.backend.clone(), &state.base_dir, &run_ctx)?;

    let registry = state.registry.clone();
    let script = spec.script;
    let task = tokio::spawn(async move {
        let _ = crate::service::run::execute(&run_ctx, prepared.runtime, script).await;
        registry.remove(&run_id);
        drop(permit);
    });

    state
        .registry
        .insert(run_id, RunHandle { events: events_tx, cancel, task });
    Ok(())
}

pub async fn handle_run(
    req_id: String,
    payload: crate::ws::protocol::RunPayload,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
    pending_confirms: &mut HashMap<RunId, (crate::service::run::RunSpec, Instant)>,
) {
    use crate::service::run::{resolve_fresh, validate_source, RunInput, ScriptSource};

    let input = RunInput {
        nl: payload.nl.clone(),
        workflow: payload.workflow.clone(),
        script: payload.script.clone(),
    };
    if validate_source(&input).is_err() {
        let _ = out_tx
            .send(ServerMsg::Error {
                req_id,
                code: ErrorCode::BadRequest,
                message: "exactly one of nl, workflow, or script must be provided".to_string(),
            })
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

    // NL + confirm: plan the script, stash it for confirmation, and return a
    // preview. The run id is fixed here so `confirm_run` reuses it.
    if payload.confirm {
        if let Some(nl) = payload.nl.as_deref() {
            match resolve_fresh(ScriptSource::Nl(nl), state.backend.clone()).await {
                Ok(mut spec) => {
                    spec.extra_args = payload.args;
                    let run_id = spec.run_id;
                    let script = spec.script.clone();
                    pending_confirms.insert(run_id, (spec, Instant::now()));
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
    }

    // Resolve the script source (NL → plan, workflow → read file, or passthrough).
    let is_nl = payload.nl.is_some();
    let source = if let Some(nl) = payload.nl.as_deref() {
        ScriptSource::Nl(nl)
    } else if let Some(wf) = payload.workflow.as_deref() {
        ScriptSource::Workflow(wf)
    } else {
        ScriptSource::Script(payload.script.as_deref().unwrap_or_default())
    };

    let mut spec = match resolve_fresh(source, state.backend.clone()).await {
        Ok(s) => s,
        Err(e) => {
            drop(permit);
            let code = if is_nl { ErrorCode::BackendError } else { ErrorCode::BadRequest };
            let _ = out_tx
                .send(ServerMsg::Error { req_id, code, message: e.to_string() })
                .await;
            return;
        }
    };
    spec.extra_args = payload.args;
    let run_id = spec.run_id;

    let (events_tx, _events_rx) = tokio::sync::broadcast::channel(256);
    let cancel = tokio_util::sync::CancellationToken::new();
    if let Err(e) = spawn_prepared(state, spec, events_tx, cancel, permit) {
        let _ = out_tx
            .send(ServerMsg::Error { req_id, code: ErrorCode::Internal, message: format!("failed to start run: {}", e) })
            .await;
        return;
    }

    let _ = out_tx.send(ServerMsg::Accepted { req_id, run_id }).await;
}

pub async fn handle_confirm_run(
    req_id: String,
    payload: crate::ws::protocol::ConfirmRunPayload,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
    pending_confirms: &mut HashMap<RunId, (crate::service::run::RunSpec, Instant)>,
) {
    let Some((spec, _ts)) = pending_confirms.remove(&payload.run_id) else {
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
        return;
    };

    if !payload.approve {
        let _ = out_tx.send(ServerMsg::Ok { req_id }).await;
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

    let (events_tx, _events_rx) = tokio::sync::broadcast::channel(256);
    let cancel = tokio_util::sync::CancellationToken::new();
    if let Err(e) = spawn_prepared(state, spec, events_tx, cancel, permit) {
        let _ = out_tx
            .send(ServerMsg::Error { req_id, code: ErrorCode::Internal, message: format!("failed to start run: {}", e) })
            .await;
        return;
    }

    let _ = out_tx.send(ServerMsg::Ok { req_id }).await;
}

pub async fn handle_resume(
    req_id: String,
    run_id: RunId,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
) {
    use crate::service::run::{check_resumable, resolve_resume, ResumeCheck};

    if state.registry.contains(&run_id) {
        let _ = out_tx
            .send(ServerMsg::Error { req_id, code: ErrorCode::AlreadyRunning, message: format!("run {} is already running", run_id) })
            .await;
        return;
    }

    match check_resumable(run_id, &state.base_dir) {
        ResumeCheck::NotFound => {
            let _ = out_tx
                .send(ServerMsg::Error { req_id, code: ErrorCode::NotFound, message: format!("run {} not found", run_id) })
                .await;
            return;
        }
        ResumeCheck::NotResumable(status) => {
            let _ = out_tx
                .send(ServerMsg::Error { req_id, code: ErrorCode::RunFinished, message: format!("run {} is not resumable (status: {:?})", run_id, status) })
                .await;
            return;
        }
        ResumeCheck::CanResume => {}
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

    // Resume is fire-and-forget: the run id is already known, so we register the
    // event/cancel handles and return `Accepted` immediately, resolving the
    // persisted script + preparing the runtime inside the spawned task. Resume
    // failures surface as a stalled run rather than a synchronous error.
    let (events_tx, _events_rx) = tokio::sync::broadcast::channel(256);
    let cancel = tokio_util::sync::CancellationToken::new();
    let run_ctx = RunContext {
        run_id,
        cancel: cancel.clone(),
        events: events_tx.clone(),
    };
    let backend = state.backend.clone();
    let base_dir = state.base_dir.clone();
    let registry = state.registry.clone();
    let task = tokio::spawn(async move {
        if let Ok(spec) = resolve_resume(run_id, &base_dir) {
            if let Ok(prepared) = crate::service::run::prepare(&spec, backend, &base_dir, &run_ctx) {
                let _ = crate::service::run::execute(&run_ctx, prepared.runtime, spec.script).await;
            }
        }
        registry.remove(&run_id);
        drop(permit);
    });

    state
        .registry
        .insert(run_id, RunHandle { events: events_tx, cancel, task });

    let _ = out_tx.send(ServerMsg::Accepted { req_id, run_id }).await;
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
    use crate::service::run::RunSpec;
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
        let mut pending = HashMap::new();
        handle_run(
            "r1".into(),
            crate::ws::protocol::RunPayload {
                nl: None, workflow: None, script: None,
                args: serde_json::Value::Null, confirm: false,
            },
            &state, &tx, &mut pending,
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
        let mut pending = HashMap::new();
        handle_run(
            "r2".into(),
            crate::ws::protocol::RunPayload {
                nl: Some("hi".into()), script: Some("print(1)".into()),
                workflow: None, args: serde_json::Value::Null, confirm: false,
            },
            &state, &tx, &mut pending,
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
        let mut pending = HashMap::new();
        handle_run(
            "r3".into(),
            crate::ws::protocol::RunPayload {
                nl: Some("hello".into()), script: None, workflow: None,
                args: serde_json::Value::Null, confirm: false,
            },
            &state, &tx, &mut pending,
        ).await;
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, .. } => assert!(matches!(code, ErrorCode::Capacity)),
            _ => panic!("expected capacity error"),
        }
    }

    #[tokio::test]
    async fn run_with_nl_confirm_planning_success() {
        let dir = std::env::temp_dir().join("maestro_test_run_confirm_ok");
        // The planner extracts a Lua script from the agent output, so the mock
        // must return one (an empty `{}` is not a valid script).
        let state = AppState {
            backend: Arc::new(MockBackend::new("test", vec![MockBehavior::Success {
                output: serde_json::json!("report({ summary = \"ok\" })"),
                tokens: Default::default(),
                delay: Duration::ZERO,
            }])),
            registry: test_state(&dir).registry,
            base_dir: test_state(&dir).base_dir,
            run_permits: test_state(&dir).run_permits,
            confirm_timeout: test_state(&dir).confirm_timeout,
        };
        let (tx, mut rx) = mpsc::channel(16);
        let mut pending = HashMap::new();
        handle_run(
            "r4".into(),
            crate::ws::protocol::RunPayload {
                nl: Some("test".into()), script: None, workflow: None,
                args: serde_json::Value::Null, confirm: true,
            },
            &state, &tx, &mut pending,
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
            kind: crate::core::FailKind::Protocol,
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
        let mut pending = HashMap::new();
        handle_run(
            "r5".into(),
            crate::ws::protocol::RunPayload {
                nl: Some("bad".into()), script: None, workflow: None,
                args: serde_json::Value::Null, confirm: true,
            },
            &state, &tx, &mut pending,
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
            kind: crate::core::FailKind::Protocol,
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
        let mut pending = HashMap::new();
        handle_run(
            "r6".into(),
            crate::ws::protocol::RunPayload {
                nl: Some("bad".into()), script: None, workflow: None,
                args: serde_json::Value::Null, confirm: false,
            },
            &state, &tx, &mut pending,
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
        let mut pending = HashMap::new();
        handle_run(
            "r7".into(),
            crate::ws::protocol::RunPayload {
                nl: None, script: None, workflow: Some("/nonexistent/workflow.lua".into()),
                args: serde_json::Value::Null, confirm: false,
            },
            &state, &tx, &mut pending,
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
        pending.insert(run_id, (RunSpec { run_id, script: "print('approved')".to_string(), task_label: String::new(), resuming: false, extra_args: serde_json::json!({}) }, std::time::Instant::now()));
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
        pending.insert(run_id, (RunSpec { run_id, script: "print('full')".to_string(), task_label: String::new(), resuming: false, extra_args: serde_json::json!({}) }, std::time::Instant::now()));
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
        pending.insert(run_id, (RunSpec { run_id, script: "print('rejected')".to_string(), task_label: String::new(), resuming: false, extra_args: serde_json::json!({}) }, std::time::Instant::now()));
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
