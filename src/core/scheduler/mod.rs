//! Concurrent scheduler (M1, §2): concurrency limiting, per-run quota, retry,
//! cancellation, and event reporting.
//!
//! Design note (§9.2 C1): the public `run_agent` returns
//! `Result<AgentResult, SchedulerError>` rather than the design doc's
//! `(AgentResult, TaskHandle)` tuple — per-agent cancellation is keyed by
//! `agent_id` via [`Scheduler::cancel_agent`], so a handle is unnecessary.

mod config;
mod error;
mod registry;

pub use config::{RetryPolicy, SchedulerConfig};
pub use error::SchedulerError;
pub use registry::BackendRegistry;

use crate::core::contract::*;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, Semaphore};
use tokio_util::sync::CancellationToken;

/// Callback invoked by the scheduler when an agent completes.
/// Implemented by JournalStore to enable transparent persistence.
/// Defined here (not in journal.rs) to avoid circular dependency:
///   scheduler → journal → scheduler.
#[async_trait::async_trait]
pub trait JournalCallback: Send + Sync {
    /// Called when an agent completes (success or non-retryable failure).
    async fn on_agent_done(
        &self,
        agent_id: AgentId,
        phase_id: PhaseId,
        status: AgentStatus,
        output: serde_json::Value,
        tokens: TokenUsage,
    );
}

/// Per-run state held inside the scheduler.
struct RunState {
    quota_used: Arc<AtomicU32>,
    run_cancel: CancellationToken,
    events: EventSender,
    /// Per-agent cancel tokens (children of `run_cancel`), keyed by agent id.
    agent_cancels: DashMap<AgentId, CancellationToken>,
}

/// Concurrency-controlled agent scheduler. Held as `Arc<Scheduler>` and shared
/// across orchestration coroutines.
pub struct Scheduler {
    config: SchedulerConfig,
    semaphore: Arc<Semaphore>,
    registry: BackendRegistry,
    runs: DashMap<RunId, RunState>,
    /// Optional journal callback invoked after each agent completes.
    /// Used by JournalStore for transparent checkpoint persistence.
    journal_callback: Option<Arc<dyn JournalCallback>>,
}

impl Scheduler {
    pub fn new(
        config: SchedulerConfig,
        registry: BackendRegistry,
        journal_callback: Option<Arc<dyn JournalCallback>>,
    ) -> Arc<Self> {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrency));
        Arc::new(Self {
            config,
            semaphore,
            registry,
            runs: DashMap::new(),
            journal_callback,
        })
    }

    pub fn config(&self) -> &SchedulerConfig {
        &self.config
    }

    /// Initialise per-run state. Must be called before any `run_agent`.
    /// Returns the broadcast receiver; further consumers use `resubscribe()`.
    pub fn init_run(&self, run_id: RunId, event_capacity: usize) -> broadcast::Receiver<AgentEvent> {
        let (tx, rx) = broadcast::channel(event_capacity);
        self.init_run_with(run_id, tx);
        rx
    }

    /// Initialise per-run state using an externally-owned event sender.
    ///
    /// This lets the orchestration layer share a single event bus between the
    /// scheduler (`AgentStarted`/`AgentDone`, plus the [`RunContext`] handed to
    /// backends) and the runtime SDK (`phase`/`log`/`pipeline`/`RunDone`).
    pub fn init_run_with(&self, run_id: RunId, events: EventSender) {
        self.runs.insert(
            run_id,
            RunState {
                quota_used: Arc::new(AtomicU32::new(0)),
                run_cancel: CancellationToken::new(),
                events,
                agent_cancels: DashMap::new(),
            },
        );
    }

    /// Schedule and run a single agent task: quota check → permit → retry loop →
    /// events. Cancellation flows via `RunContext::cancel`.
    pub async fn run_agent(
        &self,
        run_id: RunId,
        task: AgentTask,
        backend_id: Option<&str>,
    ) -> Result<AgentResult, SchedulerError> {
        let backend = match backend_id {
            Some(id) => self.registry.get(id)?,
            None => self.registry.default_backend()?,
        };

        // Snapshot per-run handles without holding the DashMap guard across await.
        let (quota_used, run_cancel, events) = {
            let rs = self
                .runs
                .get(&run_id)
                .ok_or(SchedulerError::RunNotFound(run_id))?;
            (rs.quota_used.clone(), rs.run_cancel.clone(), rs.events.clone())
        };

        // Quota.
        let used = quota_used.fetch_add(1, Ordering::Relaxed) + 1;
        if used > self.config.quota_per_run {
            return Err(SchedulerError::QuotaExceeded {
                limit: self.config.quota_per_run,
                used,
            });
        }

        // Per-agent cancel token: a child of the run token, so it fires when the
        // run is cancelled OR this agent is cancelled individually.
        let agent_token = run_cancel.child_token();
        if let Some(rs) = self.runs.get(&run_id) {
            rs.agent_cancels.insert(task.agent_id, agent_token.clone());
        }

        // Acquire a permit (cancellable while waiting).
        let permit = tokio::select! {
            p = self.semaphore.clone().acquire_owned() => p.expect("semaphore never closed"),
            _ = agent_token.cancelled() => {
                self.cleanup_agent(run_id, task.agent_id);
                return Err(cancel_kind(&run_cancel));
            }
        };

        let _ = events.send(AgentEvent::AgentStarted {
            run_id,
            phase_id: task.phase_id,
            agent_id: task.agent_id,
            prompt_preview: preview(&task.prompt),
            model: task.model.clone(),
        });

        let start = Instant::now();
        let mut attempt = 0u32;
        let outcome: Result<AgentResult, SchedulerError> = loop {
            let ctx = RunContext {
                run_id,
                cancel: agent_token.clone(),
                events: events.clone(),
            };
            let run_fut = backend.run(task.clone(), ctx);
            let res = match task.timeout {
                Some(t) => match tokio::time::timeout(t, run_fut).await {
                    Ok(r) => r,
                    Err(_) => Err(BackendError::Timeout),
                },
                None => run_fut.await,
            };

            match res {
                Ok(result) => {
                    // Validate output against schema if configured (M4).
                    if let Some(ref schema) = task.output_schema {
                        if let Err(e) = validate_output(&result.output, schema) {
                            break Err(SchedulerError::SchemaValidation(e.to_string()));
                        }
                    }
                    break Ok(result)
                },
                Err(e) => {
                    if agent_token.is_cancelled() || matches!(e, BackendError::Cancelled) {
                        break Err(cancel_kind(&run_cancel));
                    }
                    if !e.is_retryable() {
                        break Err(SchedulerError::NonRetryable(e));
                    }
                    attempt += 1;
                    if attempt > self.config.retry.max_attempts {
                        break Err(SchedulerError::Exhausted { attempts: attempt, source: e });
                    }
                    let backoff = self.config.retry.backoff(attempt);
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = agent_token.cancelled() => break Err(cancel_kind(&run_cancel)),
                    }
                }
            }
        };

        let elapsed_ms = start.elapsed().as_millis() as u64;
        let (status, tokens) = match &outcome {
            Ok(r) => (r.status.clone(), r.tokens_used),
            Err(SchedulerError::AgentCancelled) | Err(SchedulerError::RunCancelled) => {
                (AgentStatus::Cancelled, TokenUsage::default())
            }
            Err(_) => (AgentStatus::Error, TokenUsage::default()),
        };
        let _ = events.send(AgentEvent::AgentDone {
            run_id,
            agent_id: task.agent_id,
            status: status.clone(),
            tokens,
            elapsed_ms,
        });

        // Invoke journal callback if configured (M1 transparent persistence).
        if let Some(ref cb) = self.journal_callback {
            let output = match &outcome {
                Ok(r) => r.output.clone(),
                Err(_) => serde_json::Value::Null,
            };
            let agent_status = status.clone();
            let tokens_used = tokens;
            let agent_id = task.agent_id;
            let phase_id = task.phase_id;
            cb.on_agent_done(agent_id, phase_id, agent_status, output, tokens_used).await;
        }

        drop(permit);
        self.cleanup_agent(run_id, task.agent_id);
        outcome
    }

    /// Run a batch of tasks concurrently (the `parallel()` primitive). Bounded
    /// by the same global semaphore; does not short-circuit on failure — results
    /// preserve input order.
    pub async fn run_parallel(
        &self,
        run_id: RunId,
        tasks: Vec<(AgentTask, Option<String>)>,
    ) -> Vec<Result<AgentResult, SchedulerError>> {
        let futs = tasks
            .into_iter()
            .map(|(task, backend)| async move { self.run_agent(run_id, task, backend.as_deref()).await });
        futures::future::join_all(futs).await
    }

    /// Cancel one agent (fires its token; the backend observes `ctx.cancel`).
    pub fn cancel_agent(&self, run_id: RunId, agent_id: AgentId) {
        if let Some(rs) = self.runs.get(&run_id) {
            if let Some(tok) = rs.agent_cancels.get(&agent_id) {
                tok.cancel();
            }
        }
    }

    /// Cancel the whole run (all child agent tokens fire).
    pub fn cancel_run(&self, run_id: RunId) {
        if let Some(rs) = self.runs.get(&run_id) {
            rs.run_cancel.cancel();
        }
    }

    /// Current global active concurrency (for the TUI footer).
    pub fn active_concurrency(&self) -> usize {
        self.config.max_concurrency - self.semaphore.available_permits()
    }

    /// Quota consumed by a run, if initialised.
    pub fn quota_used(&self, run_id: RunId) -> Option<u32> {
        self.runs
            .get(&run_id)
            .map(|rs| rs.quota_used.load(Ordering::Relaxed))
    }

    fn cleanup_agent(&self, run_id: RunId, agent_id: AgentId) {
        if let Some(rs) = self.runs.get(&run_id) {
            rs.agent_cancels.remove(&agent_id);
        }
    }
}

fn cancel_kind(run_cancel: &CancellationToken) -> SchedulerError {
    if run_cancel.is_cancelled() {
        SchedulerError::RunCancelled
    } else {
        SchedulerError::AgentCancelled
    }
}

fn preview(s: &str) -> String {
    s.chars().take(60).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mock_backend::{FailKind, MockBackend, MockBehavior};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;
    use uuid::Uuid;

    fn fast_config(max_concurrency: usize, quota: u32) -> SchedulerConfig {
        SchedulerConfig {
            max_concurrency,
            quota_per_run: quota,
            retry: RetryPolicy {
                max_attempts: 2,
                initial_backoff: Duration::from_millis(1),
                backoff_multiplier: 2.0,
                max_backoff: Duration::from_millis(5),
            },
        }
    }

    fn mk_task(prompt: &str) -> AgentTask {
        AgentTask {
            agent_id: Uuid::now_v7(),
            phase_id: 0,
            prompt: prompt.to_string(),
            model: None,
            allowlist: None,
            workdir: PathBuf::from("."),
            mcp_endpoint: None,
            timeout: None,
            output_schema: None,
        }
    }

    fn ok_result(id: AgentId) -> AgentResult {
        AgentResult {
            agent_id: id,
            status: AgentStatus::Ok,
            output: serde_json::Value::Null,
            findings: vec![],
            tokens_used: TokenUsage::default(),
            artifacts: vec![],
            logs: LogRef::default(),
        }
    }

    fn sched_with(backend: Arc<dyn AgentBackend>, cfg: SchedulerConfig) -> Arc<Scheduler> {
        Scheduler::new(cfg, BackendRegistry::new().with(backend), None)
    }

    // A backend that records peak concurrency.
    struct ProbeBackend {
        cur: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        delay: Duration,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ProbeBackend {
        fn id(&self) -> &'static str {
            "probe"
        }
        fn capabilities(&self) -> AgentCapabilities {
            AgentCapabilities::default()
        }
        async fn run(&self, task: AgentTask, _ctx: RunContext) -> Result<AgentResult, BackendError> {
            let c = self.cur.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(c, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            self.cur.fetch_sub(1, Ordering::SeqCst);
            Ok(ok_result(task.agent_id))
        }
    }

    #[tokio::test]
    async fn test_default_config_concurrency() {
        let c = SchedulerConfig::default().max_concurrency;
        assert!((4..=16).contains(&c), "got {c}");
    }

    #[tokio::test]
    async fn test_concurrency_limit() {
        let cur = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let backend = Arc::new(ProbeBackend {
            cur: cur.clone(),
            peak: peak.clone(),
            delay: Duration::from_millis(40),
        });
        let sched = sched_with(backend, fast_config(2, 1000));
        let run_id = Uuid::now_v7();
        let _rx = sched.init_run(run_id, 256);

        let tasks: Vec<_> = (0..6).map(|i| (mk_task(&format!("t{i}")), None)).collect();
        let results = sched.run_parallel(run_id, tasks).await;

        assert!(results.iter().all(|r| r.is_ok()));
        assert!(peak.load(Ordering::SeqCst) <= 2, "peak {}", peak.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_quota_exceeded() {
        let backend = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::Value::Null,
                tokens: TokenUsage::default(),
                delay: Duration::from_millis(5),
            }],
        ));
        let sched = sched_with(backend, fast_config(8, 3));
        let run_id = Uuid::now_v7();
        let _rx = sched.init_run(run_id, 256);

        let tasks: Vec<_> = (0..4).map(|i| (mk_task(&format!("t{i}")), None)).collect();
        let results = sched.run_parallel(run_id, tasks).await;

        let ok = results.iter().filter(|r| r.is_ok()).count();
        let quota_err = results
            .iter()
            .filter(|r| matches!(r, Err(SchedulerError::QuotaExceeded { .. })))
            .count();
        assert_eq!(ok, 3);
        assert_eq!(quota_err, 1);
    }

    #[tokio::test]
    async fn test_retry_on_retryable_error() {
        let backend = Arc::new(MockBackend::new(
            "mock",
            vec![
                MockBehavior::fail(FailKind::Spawn),
                MockBehavior::fail(FailKind::Spawn),
                MockBehavior::Success {
                    output: serde_json::Value::Null,
                    tokens: TokenUsage::default(),
                    delay: Duration::ZERO,
                },
            ],
        ));
        let probe = backend.clone();
        let sched = sched_with(backend, fast_config(4, 1000));
        let run_id = Uuid::now_v7();
        let _rx = sched.init_run(run_id, 64);

        let r = sched.run_agent(run_id, mk_task("x"), None).await;
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(probe.call_count(), 3);
    }

    #[tokio::test]
    async fn test_no_retry_on_non_retryable() {
        let backend = Arc::new(MockBackend::new("mock", vec![MockBehavior::fail(FailKind::Protocol)]));
        let probe = backend.clone();
        let sched = sched_with(backend, fast_config(4, 1000));
        let run_id = Uuid::now_v7();
        let _rx = sched.init_run(run_id, 64);

        let r = sched.run_agent(run_id, mk_task("x"), None).await;
        assert!(matches!(r, Err(SchedulerError::NonRetryable(_))), "{r:?}");
        assert_eq!(probe.call_count(), 1);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let backend = Arc::new(MockBackend::new("mock", vec![MockBehavior::fail(FailKind::Spawn)]));
        let probe = backend.clone();
        let sched = sched_with(backend, fast_config(4, 1000));
        let run_id = Uuid::now_v7();
        let _rx = sched.init_run(run_id, 64);

        let r = sched.run_agent(run_id, mk_task("x"), None).await;
        assert!(matches!(r, Err(SchedulerError::Exhausted { attempts: 3, .. })), "{r:?}");
        assert_eq!(probe.call_count(), 3);
    }

    #[tokio::test]
    async fn test_cancel_run() {
        let backend = Arc::new(MockBackend::new("mock", vec![MockBehavior::Hang]));
        let sched = sched_with(backend, fast_config(8, 1000));
        let run_id = Uuid::now_v7();
        let _rx = sched.init_run(run_id, 64);

        let s2 = sched.clone();
        let handle = tokio::spawn(async move {
            let tasks: Vec<_> = (0..3).map(|i| (mk_task(&format!("h{i}")), None)).collect();
            s2.run_parallel(run_id, tasks).await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        sched.cancel_run(run_id);

        let results = handle.await.unwrap();
        assert_eq!(results.len(), 3);
        assert!(results
            .iter()
            .all(|r| matches!(r, Err(SchedulerError::RunCancelled))));
    }

    #[tokio::test]
    async fn test_cancel_agent() {
        let backend = Arc::new(MockBackend::new("mock", vec![MockBehavior::Hang]));
        let sched = sched_with(backend, fast_config(8, 1000));
        let run_id = Uuid::now_v7();
        let _rx = sched.init_run(run_id, 64);

        let task = mk_task("hang");
        let agent_id = task.agent_id;
        let s2 = sched.clone();
        let handle = tokio::spawn(async move { s2.run_agent(run_id, task, None).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        sched.cancel_agent(run_id, agent_id);

        let r = handle.await.unwrap();
        assert!(matches!(r, Err(SchedulerError::AgentCancelled)), "{r:?}");
    }

    #[tokio::test]
    async fn test_parallel_partial_failure() {
        let backend = Arc::new(MockBackend::new(
            "mock",
            vec![
                MockBehavior::Success {
                    output: serde_json::Value::Null,
                    tokens: TokenUsage::default(),
                    delay: Duration::ZERO,
                },
                MockBehavior::fail(FailKind::Protocol),
                MockBehavior::Success {
                    output: serde_json::Value::Null,
                    tokens: TokenUsage::default(),
                    delay: Duration::ZERO,
                },
            ],
        ));
        let sched = sched_with(backend, fast_config(1, 1000)); // serialize for deterministic behavior order
        let run_id = Uuid::now_v7();
        let _rx = sched.init_run(run_id, 64);

        let tasks: Vec<_> = (0..3).map(|i| (mk_task(&format!("p{i}")), None)).collect();
        let results = sched.run_parallel(run_id, tasks).await;

        assert_eq!(results.len(), 3);
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 2);
        assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
    }

    #[tokio::test]
    async fn test_event_sequence() {
        let backend = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::Value::Null,
                tokens: TokenUsage {
                    input: 10,
                    output: 5,
                    ..Default::default()
                },
                delay: Duration::ZERO,
            }],
        ));
        let sched = sched_with(backend, fast_config(4, 1000));
        let run_id = Uuid::now_v7();
        let mut rx = sched.init_run(run_id, 64);

        let r = sched.run_agent(run_id, mk_task("x"), None).await;
        assert!(r.is_ok());

        let e1 = rx.recv().await.unwrap();
        assert!(matches!(e1, AgentEvent::AgentStarted { .. }), "{e1:?}");
        let e2 = rx.recv().await.unwrap();
        match e2 {
            AgentEvent::AgentDone { status, tokens, .. } => {
                assert_eq!(status, AgentStatus::Ok);
                assert_eq!(tokens.input, 10);
            }
            other => panic!("expected AgentDone, got {other:?}"),
        }
    }
}
