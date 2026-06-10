//! 事件流轮询与过滤。
//!
//! 由 connection.rs 主循环中的 tokio::select! 分支调用，
//! 非阻塞地轮询所有活跃 subscription 的事件流。
//!
//! ## 工作机制
//! - poll_subscriptions 遍历所有 subscription，对每个 BroadcastStream 调用 next().now_or_never()
//! - 如果事件通过 passes_filter（类型白名单匹配），返回 (run_id, event) 给 connection 层转发
//! - 如果所有 stream 暂时无数据，sleep 10ms 后返回 None（避免 busy loop）
//! - subscription 为空时进入 pending() 永久挂起（直到新订阅加入）
//!
//! ## 过滤
//! passes_filter 将 AgentEvent 的 serde tag 名与客户端指定的类型列表匹配。
//! 例如 filter: ["phase_started", "agent_done"] 只接收这两种事件。
use crate::core::contract::event::AgentEvent;
use crate::core::contract::ids::RunId;
use crate::ws::protocol::event_type_name;

use super::Subscription;

use futures::FutureExt;
use futures::StreamExt;
use std::collections::HashMap;
use std::time::Duration;

pub async fn poll_subscriptions(
    subscriptions: &mut HashMap<RunId, Subscription>,
) -> Option<(RunId, AgentEvent)> {
    if subscriptions.is_empty() {
        std::future::pending::<()>().await;
        return None;
    }

    let run_ids: Vec<RunId> = subscriptions.keys().copied().collect();
    for run_id in run_ids {
        if let Some(sub) = subscriptions.get_mut(&run_id) {
            if let Some(result) = sub.stream.next().now_or_never() {
                match result {
                    Some(Ok(evt)) => {
                        if passes_filter(&evt, &sub.filter) {
                            return Some((run_id, evt));
                        }
                    }
                    Some(Err(e)) => {
                        tracing::warn!("broadcast stream error for run {}: {:?}", run_id, e);
                    }
                    None => {}
                }
            }
        }
    }

    tokio::time::sleep(Duration::from_millis(10)).await;
    None
}

fn passes_filter(evt: &AgentEvent, filter: &Option<Vec<String>>) -> bool {
    match filter {
        None => true,
        Some(types) => {
            let name = event_type_name(evt);
            types.iter().any(|t| t == name)
        }
    }
}

#[allow(dead_code)]
fn event_run_id(evt: &AgentEvent) -> RunId {
    match evt {
        AgentEvent::RunStarted { run_id, .. } => *run_id,
        AgentEvent::PhaseStarted { run_id, .. } => *run_id,
        AgentEvent::AgentStarted { run_id, .. } => *run_id,
        AgentEvent::AgentProgress { run_id, .. } => *run_id,
        AgentEvent::AgentDone { run_id, .. } => *run_id,
        AgentEvent::PhaseDone { run_id, .. } => *run_id,
        AgentEvent::RunDone { run_id, .. } => *run_id,
        AgentEvent::Log { run_id, .. } => *run_id,
        AgentEvent::PipelineStarted { run_id, .. } => *run_id,
        AgentEvent::PipelineStageStarted { run_id, .. } => *run_id,
        AgentEvent::PipelineItemDone { run_id, .. } => *run_id,
        AgentEvent::PipelineDone { run_id, .. } => *run_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::backend::AgentStatus;
    use crate::core::contract::event::{LogLevel, ProgressDelta, RunStatus};
    use crate::core::contract::ids::TokenUsage;
    use chrono::Utc;
    use std::collections::HashMap;

    fn sample_run_started(run_id: RunId) -> AgentEvent {
        AgentEvent::RunStarted {
            run_id,
            task: "test".into(),
            ts: Utc::now(),
        }
    }

    #[tokio::test]
    async fn poll_empty_subscriptions_returns_none() {
        let mut subs: HashMap<RunId, Subscription> = HashMap::new();
        tokio::select! {
            _ = poll_subscriptions(&mut subs) => panic!("should not return"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }
    }

    #[tokio::test]
    async fn poll_subscriptions_with_event() {
        let run_id = RunId::now_v7();
        let (tx, rx) = tokio::sync::broadcast::channel(16);
        let evt = sample_run_started(run_id);
        tx.send(evt.clone()).unwrap();
        let mut subs = HashMap::new();
        subs.insert(run_id, Subscription {
            filter: None,
            stream: tokio_stream::wrappers::BroadcastStream::new(rx),
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            poll_subscriptions(&mut subs),
        ).await.unwrap();
        assert!(result.is_some());
        let (rid, _) = result.unwrap();
        assert_eq!(rid, run_id);
    }

    #[test]
    fn passes_filter_none_passes_all() {
        let evt = sample_run_started(RunId::now_v7());
        assert!(passes_filter(&evt, &None));
    }

    #[test]
    fn passes_filter_matching() {
        let evt = sample_run_started(RunId::now_v7());
        assert!(passes_filter(&evt, &Some(vec!["run_started".into()])));
    }

    #[test]
    fn passes_filter_not_matching() {
        let evt = sample_run_started(RunId::now_v7());
        assert!(!passes_filter(&evt, &Some(vec!["agent_done".into()])));
    }

    #[test]
    fn event_run_id_all_variants() {
        let run_id = RunId::now_v7();
        let cases: Vec<AgentEvent> = vec![
            AgentEvent::RunStarted { run_id, task: "t".into(), ts: Utc::now() },
            AgentEvent::PhaseStarted { run_id, phase_id: 0, label: "p".into(), planned: 1 },
            AgentEvent::AgentStarted { run_id, phase_id: 0, agent_id: run_id, prompt_preview: "p".into(), model: None },
            AgentEvent::AgentProgress { run_id, agent_id: run_id, delta: ProgressDelta::Message { text: "d".into() } },
            AgentEvent::AgentDone { run_id, agent_id: run_id, status: AgentStatus::Ok, tokens: TokenUsage::default(), elapsed_ms: 0 },
            AgentEvent::PhaseDone { run_id, phase_id: 0, ok: 1, failed: 0 },
            AgentEvent::RunDone { run_id, status: RunStatus::Completed, total_tokens: TokenUsage::default(), report: serde_json::json!(null) },
            AgentEvent::Log { run_id, agent_id: None, level: LogLevel::Info, msg: "m".into() },
            AgentEvent::PipelineStarted { run_id, total_stages: 1, items: 1 },
            AgentEvent::PipelineStageStarted { run_id, stage_index: 0, label: "s".into(), agents_in_stage: 1 },
            AgentEvent::PipelineItemDone { run_id, stage_index: 0, item_index: 0, status: AgentStatus::Ok, tokens: TokenUsage::default(), elapsed_ms: 0 },
            AgentEvent::PipelineDone { run_id, stages_completed: 1, total_ok: 1, total_failed: 0 },
        ];
        for evt in &cases {
            assert_eq!(event_run_id(evt), run_id);
        }
    }

    #[tokio::test]
    async fn poll_subscriptions_broadcast_stream_error() {
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
        drop(tx); // Drop sender to cause broadcast stream errors
        let mut subs = HashMap::new();
        subs.insert(run_id, Subscription {
            filter: None,
            stream: tokio_stream::wrappers::BroadcastStream::new(
                tokio::sync::broadcast::channel(16).1
            ),
        });
        
        // This should handle the broadcast stream error gracefully
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            poll_subscriptions(&mut subs),
        ).await;
        
        // The function should return None after handling the error
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn poll_subscriptions_no_immediate_events() {
        let run_id = RunId::now_v7();
        let (_tx, rx) = tokio::sync::broadcast::channel(16);
        let mut subs = HashMap::new();
        subs.insert(run_id, Subscription {
            filter: None,
            stream: tokio_stream::wrappers::BroadcastStream::new(rx),
        });
        
        // Don't send any events, should return None after sleep
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            poll_subscriptions(&mut subs),
        ).await.unwrap();
        
        assert!(result.is_none());
    }
}
