//! `MockBackend` (§2.8) — a deterministic test backend. Compiled under `test`
//! or the `testing` feature so integration tests can reuse it.

use crate::contract::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How a mocked `run()` call should behave. Behaviours are consumed by call
/// order; the last entry repeats once exhausted.
#[derive(Debug, Clone)]
pub enum MockBehavior {
    Success {
        output: serde_json::Value,
        tokens: TokenUsage,
        delay: Duration,
    },
    /// Fail with a fresh [`BackendError`] of the given kind (errors aren't
    /// `Clone`, so we reconstruct on each call).
    Fail { kind: FailKind, delay: Duration },
    /// Never returns until cancelled; then yields `BackendError::Cancelled`.
    Hang,
}

impl MockBehavior {
    /// Convenience constructor for a zero-delay failure.
    pub fn fail(kind: FailKind) -> Self {
        MockBehavior::Fail {
            kind,
            delay: Duration::ZERO,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailKind {
    Spawn,
    Protocol,
    Timeout,
    Cancelled,
}

impl FailKind {
    fn to_error(self) -> BackendError {
        match self {
            FailKind::Spawn => BackendError::Spawn("mock spawn".into()),
            FailKind::Protocol => BackendError::Protocol("mock protocol".into()),
            FailKind::Timeout => BackendError::Timeout,
            FailKind::Cancelled => BackendError::Cancelled,
        }
    }
}

#[derive(Clone)]
pub struct MockBackend {
    id: &'static str,
    behaviors: Vec<MockBehavior>,
    calls: Arc<AtomicU32>,
}

impl MockBackend {
    pub fn new(id: &'static str, behaviors: Vec<MockBehavior>) -> Self {
        assert!(
            !behaviors.is_empty(),
            "MockBackend needs at least one behavior"
        );
        Self {
            id,
            behaviors,
            calls: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Number of times `run()` has been invoked.
    pub fn call_count(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }

    fn next(&self) -> &MockBehavior {
        let i = (self.calls.fetch_add(1, Ordering::SeqCst) as usize).min(self.behaviors.len() - 1);
        &self.behaviors[i]
    }
}

#[async_trait::async_trait]
impl AgentBackend for MockBackend {
    fn id(&self) -> &'static str {
        self.id
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities::default()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn run(&self, task: AgentTask, ctx: RunContext) -> Result<AgentResult, BackendError> {
        match self.next() {
            MockBehavior::Success {
                output,
                tokens,
                delay,
            } => {
                tokio::select! {
                    _ = tokio::time::sleep(*delay) => {}
                    _ = ctx.cancel.cancelled() => return Err(BackendError::Cancelled),
                }
                Ok(AgentResult {
                    agent_id: task.agent_id,
                    status: AgentStatus::Ok,
                    output: output.clone(),
                    findings: vec![],
                    tokens_used: *tokens,
                    artifacts: vec![],
                    logs: LogRef::default(),
                })
            }
            MockBehavior::Fail { kind, delay } => {
                tokio::select! {
                    _ = tokio::time::sleep(*delay) => {}
                    _ = ctx.cancel.cancelled() => return Err(BackendError::Cancelled),
                }
                Err(kind.to_error())
            }
            MockBehavior::Hang => {
                ctx.cancel.cancelled().await;
                Err(BackendError::Cancelled)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::backend::AgentTask;
    use crate::contract::ids::AgentId;
    use serde_json::json;
    use std::path::PathBuf;
    use tokio::sync::broadcast;
    use uuid::Uuid;

    fn fresh_ctx() -> RunContext {
        let (tx, _rx) = broadcast::channel(4);
        RunContext {
            run_id: Uuid::nil(),
            cancel: tokio_util::sync::CancellationToken::new(),
            events: tx,
        }
    }

    fn make_ctx() -> RunContext {
        fresh_ctx()
    }

    fn make_task() -> AgentTask {
        AgentTask {
            agent_id: Uuid::nil(),
            phase_id: 0,
            prompt: "test".into(),
            model: None,
            description: None,
            role: None,
            name: None,
            agent_seq: 0,
            allowlist: Some(ToolPolicy::default()),
            workdir: PathBuf::from("."),
            mcp_endpoint: None,
            timeout: None,
            output_schema: None,
        }
    }

    // ── Construction & accessors ─────────────────────────────────

    #[test]
    #[should_panic(expected = "needs at least one behavior")]
    fn new_with_empty_behaviors_panics() {
        let _ = MockBackend::new("mock", vec![]);
    }

    #[test]
    fn id_and_capabilities_are_returned() {
        let b = MockBackend::new(
            "mock-id",
            vec![MockBehavior::Success {
                output: json!(null),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        );
        assert_eq!(b.id(), "mock-id");
        let _caps = b.capabilities();
        // as_any roundtrip
        let _ = b.as_any().is::<MockBackend>();
    }

    #[test]
    fn call_count_starts_at_zero() {
        let b = MockBackend::new("m", vec![MockBehavior::Hang]);
        assert_eq!(b.call_count(), 0);
    }

    // ── Success path ─────────────────────────────────────────────

    #[tokio::test]
    async fn success_returns_ok_with_provided_output_and_tokens() {
        let tokens = TokenUsage {
            input: 10,
            output: 20,
            cache_read: 0,
            cache_write: 0,
        };
        let b = MockBackend::new(
            "m",
            vec![MockBehavior::Success {
                output: json!({"answer": 42}),
                tokens,
                delay: Duration::ZERO,
            }],
        );
        let res = b.run(make_task(), make_ctx()).await.unwrap();
        assert_eq!(res.status, AgentStatus::Ok);
        assert_eq!(res.output, json!({"answer": 42}));
        assert_eq!(res.tokens_used, tokens);
        assert_eq!(res.agent_id, AgentId::nil());
        assert!(res.findings.is_empty());
        assert!(res.artifacts.is_empty());
        assert_eq!(
            serde_json::to_string(&res.logs).unwrap(),
            serde_json::to_string(&LogRef::default()).unwrap()
        );
        assert_eq!(b.call_count(), 1);
    }

    #[tokio::test]
    async fn success_respects_delay_then_returns() {
        let start = std::time::Instant::now();
        let b = MockBackend::new(
            "m",
            vec![MockBehavior::Success {
                output: json!(null),
                tokens: TokenUsage::default(),
                delay: Duration::from_millis(40),
            }],
        );
        let res = b.run(make_task(), make_ctx()).await.unwrap();
        assert_eq!(res.status, AgentStatus::Ok);
        assert!(start.elapsed() >= Duration::from_millis(35));
    }

    #[tokio::test]
    async fn success_yields_cancelled_when_cancel_signal_arrives() {
        let b = MockBackend::new(
            "m",
            vec![MockBehavior::Success {
                output: json!(null),
                tokens: TokenUsage::default(),
                delay: Duration::from_millis(500),
            }],
        );
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = RunContext {
            run_id: Uuid::nil(),
            cancel: cancel.clone(),
            events: broadcast::channel(8).0,
        };
        let ctx2 = ctx.clone();
        let h = tokio::spawn(async move { b.run(make_task(), ctx2).await });
        // Let the task start its sleep.
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        let res = h.await.unwrap();
        assert!(matches!(res, Err(BackendError::Cancelled)));
    }

    // ── Failure path ─────────────────────────────────────────────

    #[tokio::test]
    async fn fail_spawn_returns_spawn_error() {
        let b = MockBackend::new("m", vec![MockBehavior::fail(FailKind::Spawn)]);
        let res = b.run(make_task(), make_ctx()).await;
        assert!(matches!(res, Err(BackendError::Spawn(_))));
    }

    #[tokio::test]
    async fn fail_protocol_returns_protocol_error() {
        let b = MockBackend::new("m", vec![MockBehavior::fail(FailKind::Protocol)]);
        let res = b.run(make_task(), make_ctx()).await;
        assert!(matches!(res, Err(BackendError::Protocol(_))));
    }

    #[tokio::test]
    async fn fail_timeout_returns_timeout_error() {
        let b = MockBackend::new("m", vec![MockBehavior::fail(FailKind::Timeout)]);
        let res = b.run(make_task(), make_ctx()).await;
        assert!(matches!(res, Err(BackendError::Timeout)));
    }

    #[tokio::test]
    async fn fail_cancelled_returns_cancelled_error() {
        let b = MockBackend::new("m", vec![MockBehavior::fail(FailKind::Cancelled)]);
        let res = b.run(make_task(), make_ctx()).await;
        assert!(matches!(res, Err(BackendError::Cancelled)));
    }

    #[tokio::test]
    async fn fail_with_delay_yields_cancelled_if_cancelled_during_delay() {
        let b = MockBackend::new(
            "m",
            vec![MockBehavior::Fail {
                kind: FailKind::Spawn,
                delay: Duration::from_millis(500),
            }],
        );
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = RunContext {
            run_id: Uuid::nil(),
            cancel: cancel.clone(),
            events: broadcast::channel(8).0,
        };
        let ctx2 = ctx.clone();
        let b_clone = b.clone();
        let h = tokio::spawn(async move { b_clone.run(make_task(), ctx2).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        let res = h.await.unwrap();
        // Cancelled during delay takes priority over the eventual fail error.
        assert!(matches!(res, Err(BackendError::Cancelled)));
    }

    // ── Hang path ────────────────────────────────────────────────

    #[tokio::test]
    async fn hang_only_resolves_on_cancellation() {
        let b = MockBackend::new("m", vec![MockBehavior::Hang]);
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = RunContext {
            run_id: Uuid::nil(),
            cancel: cancel.clone(),
            events: broadcast::channel(8).0,
        };
        let ctx2 = ctx.clone();
        let b_clone = b.clone();
        let h = tokio::spawn(async move { b_clone.run(make_task(), ctx2).await });
        // Confirm it really is hanging — nothing happens in 50ms.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!h.is_finished());
        cancel.cancel();
        let res = h.await.unwrap();
        assert!(matches!(res, Err(BackendError::Cancelled)));
        assert_eq!(b.call_count(), 1);
    }

    // ── Behavior sequencing ──────────────────────────────────────

    #[tokio::test]
    async fn behaviors_are_consumed_in_order() {
        let b = MockBackend::new(
            "m",
            vec![
                MockBehavior::fail(FailKind::Spawn),
                MockBehavior::Success {
                    output: json!({"step": 2}),
                    tokens: TokenUsage::default(),
                    delay: Duration::ZERO,
                },
                MockBehavior::fail(FailKind::Timeout),
            ],
        );
        let r1 = b.run(make_task(), make_ctx()).await;
        assert!(matches!(r1, Err(BackendError::Spawn(_))));
        let r2 = b.run(make_task(), make_ctx()).await.unwrap();
        assert_eq!(r2.output, json!({"step": 2}));
        let r3 = b.run(make_task(), make_ctx()).await;
        assert!(matches!(r3, Err(BackendError::Timeout)));
        assert_eq!(b.call_count(), 3);
    }

    #[tokio::test]
    async fn last_behavior_repeats_after_list_exhausted() {
        // After 3 behaviors, the 4th call must repeat behavior #2 (Hang).
        let b = MockBackend::new(
            "m",
            vec![
                MockBehavior::Success {
                    output: json!(1),
                    tokens: TokenUsage::default(),
                    delay: Duration::ZERO,
                },
                MockBehavior::Success {
                    output: json!(2),
                    tokens: TokenUsage::default(),
                    delay: Duration::ZERO,
                },
                MockBehavior::Hang,
            ],
        );
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = RunContext {
            run_id: Uuid::nil(),
            cancel: cancel.clone(),
            events: broadcast::channel(8).0,
        };
        let r1 = b.run(make_task(), make_ctx()).await.unwrap();
        assert_eq!(r1.output, json!(1));
        let r2 = b.run(make_task(), make_ctx()).await.unwrap();
        assert_eq!(r2.output, json!(2));
        // 3rd call should hang.
        let ctx2 = ctx.clone();
        let b_for_spawn = b.clone();
        let h = tokio::spawn(async move { b_for_spawn.run(make_task(), ctx2).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!h.is_finished());
        cancel.cancel();
        let r3 = h.await.unwrap();
        assert!(matches!(r3, Err(BackendError::Cancelled)));
        // 4th call must still hang (last behavior repeats).
        let ctx3 = tokio_util::sync::CancellationToken::new();
        let ctx3_clone = ctx3.clone();
        let b_for_spawn2 = b.clone();
        let h2 = tokio::spawn(async move {
            let c = RunContext {
                run_id: Uuid::nil(),
                cancel: ctx3_clone,
                events: broadcast::channel(8).0,
            };
            b_for_spawn2.run(make_task(), c).await
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!h2.is_finished());
        ctx3.cancel();
        let r4 = h2.await.unwrap();
        assert!(matches!(r4, Err(BackendError::Cancelled)));
    }

    #[tokio::test]
    async fn single_behavior_repeats_forever() {
        let b = MockBackend::new(
            "m",
            vec![MockBehavior::Success {
                output: json!("ok"),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        );
        for i in 1..=5 {
            let r = b.run(make_task(), make_ctx()).await.unwrap();
            assert_eq!(r.output, json!("ok"));
            assert_eq!(b.call_count(), i);
        }
    }

    #[tokio::test]
    async fn call_count_increments_even_when_behavior_is_hang() {
        let b = MockBackend::new("m", vec![MockBehavior::Hang]);
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = RunContext {
            run_id: Uuid::nil(),
            cancel: cancel.clone(),
            events: broadcast::channel(8).0,
        };
        let ctx2 = ctx.clone();
        let b_clone = b.clone();
        let h = tokio::spawn(async move { b_clone.run(make_task(), ctx2).await });
        // The call counter is incremented synchronously before the await,
        // so by the time the spawned task is waiting on cancellation,
        // the count is already 1.
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(b.call_count(), 1);
        cancel.cancel();
        let _ = h.await.unwrap();
        assert_eq!(b.call_count(), 1);
    }

    // ── MockBehavior helper ──────────────────────────────────────

    #[test]
    fn mock_behavior_fail_helper_uses_zero_delay() {
        let b = MockBehavior::fail(FailKind::Spawn);
        match b {
            MockBehavior::Fail { kind, delay } => {
                assert!(matches!(kind, FailKind::Spawn));
                assert_eq!(delay, Duration::ZERO);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mock_behavior_clone_is_independent() {
        let b = MockBehavior::Success {
            output: json!({"x": 1}),
            tokens: TokenUsage {
                input: 1,
                output: 2,
                cache_read: 0,
                cache_write: 0,
            },
            delay: Duration::from_millis(50),
        };
        let c = b.clone();
        match (b, c) {
            (
                MockBehavior::Success {
                    output: o1,
                    tokens: t1,
                    delay: d1,
                },
                MockBehavior::Success {
                    output: o2,
                    tokens: t2,
                    delay: d2,
                },
            ) => {
                assert_eq!(o1, o2);
                assert_eq!(t1, t2);
                assert_eq!(d1, d2);
            }
            _ => panic!("not success"),
        }
    }

    #[test]
    fn fail_kind_is_copy_and_eq() {
        let k = FailKind::Spawn;
        let k2 = k;
        assert_eq!(k, k2);
        assert_ne!(FailKind::Spawn, FailKind::Protocol);
    }

    #[test]
    fn mock_behavior_debug_format_is_human_readable() {
        let b = MockBehavior::Success {
            output: json!({"k": "v"}),
            tokens: TokenUsage::default(),
            delay: Duration::from_millis(100),
        };
        let dbg = format!("{:?}", b);
        assert!(dbg.contains("Success"));
        assert!(dbg.contains("delay"));
    }
}
