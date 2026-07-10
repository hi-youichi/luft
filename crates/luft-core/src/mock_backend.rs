//! `MockBackend` (§2.8) — a deterministic test backend. Compiled under `test`
//! or the `testing` feature so integration tests can reuse it.

use crate::contract::*;
use std::sync::atomic::{AtomicU32, Ordering};
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

#[derive(Debug, Clone, Copy)]
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

pub struct MockBackend {
    id: &'static str,
    behaviors: Vec<MockBehavior>,
    calls: AtomicU32,
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
            calls: AtomicU32::new(0),
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
