//! broadcast → mpsc 桥接 task。
//!
//! broadcast 的 `recv()` 需要 `.await`（异步），但渲染循环需要同步访问
//! `AppState`。桥接 task 将 broadcast 事件转为 mpsc 消息，渲染循环用
//! `select!` 消费。

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::core::contract::event::AgentEvent;

/// 从 broadcast 桥接到渲染循环的消息。
pub enum TuiMsg {
    /// AgentEvent（非 AcpRaw——桥接层已过滤高频帧）。
    Event(AgentEvent),
    /// 广播通道延迟丢包。
    Lagged(u64),
    /// 广播通道关闭（运行结束）。
    Closed,
}

/// 启动桥接 task：broadcast → mpsc。
pub fn spawn_bridge(
    mut broadcast_rx: broadcast::Receiver<AgentEvent>,
    mpsc_tx: mpsc::Sender<TuiMsg>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok(AgentEvent::AcpRaw { .. }) => {
                    // 跳过高频原始 ACP 帧
                }
                Ok(evt) => {
                    if mpsc_tx.send(TuiMsg::Event(evt)).await.is_err() {
                        break; // 渲染循环已退出
                    }
                }
                Err(broadcast::error::RecvError::Closed) => {
                    let _ = mpsc_tx.send(TuiMsg::Closed).await;
                    break;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let _ = mpsc_tx.send(TuiMsg::Lagged(n)).await;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::event::{RunStatus};
    use crate::core::contract::ids::{RunId, TokenUsage};

    #[tokio::test]
    async fn bridge_forwards_events_and_closes() {
        let (btx, brx) = broadcast::channel(16);
        let (mtx, mut mrx) = mpsc::channel(16);

        let handle = spawn_bridge(brx, mtx);

        let run_id = RunId::now_v7();
        btx.send(AgentEvent::RunStarted {
            run_id,
            task: "test".into(),
            ts: chrono::Utc::now(),
        })
        .unwrap();

        let msg = mrx.recv().await.unwrap();
        assert!(matches!(msg, TuiMsg::Event(AgentEvent::RunStarted { .. })));

        // 发送 RunDone → 关闭 broadcast → bridge 发送 Closed
        btx.send(AgentEvent::RunDone {
            run_id,
            status: RunStatus::Completed,
            total_tokens: TokenUsage::default(),
            report: serde_json::json!({}),
        })
        .unwrap();
        // 消费 RunDone
        let _ = mrx.recv().await.unwrap();
        // 关闭发送端
        drop(btx);
        // 应收到 Closed
        let msg = mrx.recv().await.unwrap();
        assert!(matches!(msg, TuiMsg::Closed));

        let _ = handle.await;
    }

    #[tokio::test]
    async fn bridge_filters_acp_raw() {
        let (btx, brx) = broadcast::channel(16);
        let (mtx, mut mrx) = mpsc::channel(16);

        let handle = spawn_bridge(brx, mtx);

        let run_id = RunId::now_v7();
        btx.send(AgentEvent::AcpRaw {
            run_id,
            agent_id: run_id,
            kind: "plan".into(),
            raw: serde_json::json!({}),
        })
        .unwrap();
        btx.send(AgentEvent::RunStarted {
            run_id,
            task: "test".into(),
            ts: chrono::Utc::now(),
        })
        .unwrap();

        // AcpRaw 应被过滤——只收到 RunStarted
        let msg = mrx.recv().await.unwrap();
        assert!(matches!(msg, TuiMsg::Event(AgentEvent::RunStarted { .. })));

        drop(btx);
        let _ = handle.await;
    }
}
