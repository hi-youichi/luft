//! 查询类 handler — run 状态查询、日志、findings、report。
//!
//! 每个 handler 都是薄适配层，职责:
//! 1. 从 state 中取出 ase_dir
//! 2. 调用 crate::service::query 中对应的业务函数
//! 3. 将结果映射为 ServerMsg（成功/NotFound/Internal error）
//!
//! 依赖链: query handler → service::query → core::state
//!
//! **不依赖** crate::cli（已在 service 层重构中移除）。
use crate::service::query;
use crate::core::contract::event::AgentEvent;
use crate::core::contract::ids::RunId;
use crate::ws::protocol::{ErrorCode, ServerMsg};

use super::AppState;

use tokio::sync::mpsc;

pub async fn handle_get_status(
    req_id: String,
    run_id: RunId,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
) {
    match query::get_status(run_id, &state.base_dir) {
        Ok(Some(status)) => {
            let _ = out_tx
                .send(ServerMsg::Status {
                    req_id,
                    run_id,
                    data: status,
                })
                .await;
        }
        Ok(None) => {
            let _ = out_tx
                .send(ServerMsg::Error {
                    req_id,
                    code: ErrorCode::NotFound,
                    message: format!("run {} not found", run_id),
                })
                .await;
        }
        Err(e) => {
            let _ = out_tx
                .send(ServerMsg::Error {
                    req_id,
                    code: ErrorCode::Internal,
                    message: e.to_string(),
                })
                .await;
        }
    }
}

pub async fn handle_list_runs(
    req_id: String,
    limit: usize,
    offset: usize,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
) {
    match query::list_runs(&state.base_dir) {
        Ok(all_runs) => {
            let total = all_runs.len();
            let items: Vec<_> = all_runs.into_iter().skip(offset).take(limit).collect();
            let _ = out_tx
                .send(ServerMsg::RunList {
                    req_id,
                    total,
                    items,
                })
                .await;
        }
        Err(e) => {
            let _ = out_tx
                .send(ServerMsg::Error {
                    req_id,
                    code: ErrorCode::Internal,
                    message: e.to_string(),
                })
                .await;
        }
    }
}

pub async fn handle_get_logs(
    req_id: String,
    run_id: RunId,
    limit: usize,
    offset: usize,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
) {
    match query::get_logs(run_id, &state.base_dir, Some(limit + offset)) {
        Ok(log_strings) => {
            let total = log_strings.len();
            let items: Vec<AgentEvent> = log_strings
                .into_iter()
                .skip(offset)
                .take(limit)
                .filter_map(|s| serde_json::from_str::<AgentEvent>(&s).ok())
                .collect();
            let _ = out_tx
                .send(ServerMsg::Logs {
                    req_id,
                    run_id,
                    total,
                    items,
                })
                .await;
        }
        Err(e) => {
            let _ = out_tx
                .send(ServerMsg::Error {
                    req_id,
                    code: super::resolve_run_error(&run_id, &state.base_dir, e.to_string()),
                    message: e.to_string(),
                })
                .await;
        }
    }
}

pub async fn handle_get_findings(
    req_id: String,
    run_id: RunId,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
) {
    match query::get_findings(run_id, &state.base_dir) {
        Ok(items) => {
            let _ = out_tx
                .send(ServerMsg::Findings {
                    req_id,
                    run_id,
                    items,
                })
                .await;
        }
        Err(e) => {
            let _ = out_tx
                .send(ServerMsg::Error {
                    req_id,
                    code: super::resolve_run_error(&run_id, &state.base_dir, e.to_string()),
                    message: e.to_string(),
                })
                .await;
        }
    }
}

pub async fn handle_get_report(
    req_id: String,
    run_id: RunId,
    state: &AppState,
    out_tx: &mpsc::Sender<ServerMsg>,
) {
    match query::get_report(run_id, &state.base_dir) {
        Ok(query::ReportStatus::Found(data)) => {
            let _ = out_tx
                .send(ServerMsg::Report { req_id, run_id, data })
                .await;
        }
        Ok(query::ReportStatus::NotFound) => {
            let _ = out_tx
                .send(ServerMsg::Error {
                    req_id,
                    code: ErrorCode::NotFound,
                    message: format!("no report found for run {}", run_id),
                })
                .await;
        }
        Ok(query::ReportStatus::RunFinished) => {
            let _ = out_tx
                .send(ServerMsg::Error {
                    req_id,
                    code: ErrorCode::RunFinished,
                    message: format!("run {} not found", run_id),
                })
                .await;
        }
        Err(e) => {
            let _ = out_tx
                .send(ServerMsg::Error {
                    req_id,
                    code: ErrorCode::Internal,
                    message: format!("failed to read events: {}", e),
                })
                .await;
        }
    }
}
