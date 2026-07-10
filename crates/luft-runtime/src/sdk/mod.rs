//! SDK primitives bridging Lua orchestration scripts to the scheduler.
//!
//! Each submodule registers one logical group of SDK globals via a
//! `register_*_sdk(lua, cx)` function (the same pattern as
//! [`crate::converge::register_converge_sdk`]). All shared
//! dependencies travel through [`SdkContext`] so the individual registrars keep
//! short, uniform signatures.

pub(crate) mod agent;
pub(crate) mod control;
pub(crate) mod convert;
pub(crate) mod report;
pub(crate) mod task;
pub(crate) mod workflow;

use luft_core::contract::backend::{AgentTask, RunContext};
use luft_core::contract::event::EventSender;
use luft_core::contract::ids::{AgentId, PhaseId, RunId};
use luft_core::journal::{AgentCacheKey, JournalStore};
use luft_core::Scheduler;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::runtime::Handle;

/// Report sink shared between the runtime and the `report()` primitive.
pub(crate) type ReportSink = Arc<Mutex<Option<serde_json::Value>>>;

/// A task yielded by a coroutine inside `pmap()`, waiting for the driver to
/// dispatch it to the scheduler.
pub(crate) struct PendingTask {
    pub task: AgentTask,
    pub backend: Option<String>,
    pub cache_key: AgentCacheKey,
    pub agent_id: AgentId,
    pub phase_id: PhaseId,
}

/// Shared state between `agent()` and `pmap()` for coroutine coordination.
///
/// When `pmap()` is active, `agent()` detects it's running inside a coroutine
/// and yields instead of calling `block_on`. The yielded request id lets the
/// `pmap()` driver retrieve the pending task from this map, dispatch it
/// asynchronously, and resume the coroutine with the result.
pub(crate) struct CoroutineBridge {
    counter: AtomicU64,
    pending: Mutex<HashMap<u64, PendingTask>>,
    in_pmap: AtomicBool,
}

impl CoroutineBridge {
    pub(crate) fn new() -> Self {
        Self {
            counter: AtomicU64::new(0),
            pending: Mutex::new(HashMap::new()),
            in_pmap: AtomicBool::new(false),
        }
    }

    pub(crate) fn enter_pmap(&self) {
        self.in_pmap
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn exit_pmap(&self) {
        self.in_pmap
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn is_in_pmap(&self) -> bool {
        self.in_pmap.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Store a pending task and return its request id for yielding.
    pub(crate) fn deposit(&self, task: PendingTask) -> u64 {
        let id = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        self.pending.lock().unwrap().insert(id, task);
        id
    }

    /// Take a pending task by request id (returns None if already consumed).
    pub(crate) fn take(&self, id: u64) -> Option<PendingTask> {
        self.pending.lock().unwrap().remove(&id)
    }
}

/// Shared dependencies captured by the SDK closures during registration.
///
/// Built once per [`Runtime`](crate::Runtime) and passed by reference
/// to each `register_*_sdk` function, which clones out only the fields its
/// closures need.
pub(crate) struct SdkContext {
    /// Full run context — `workflow`/`converge` need the whole thing.
    pub run_ctx: RunContext,
    pub scheduler: Arc<Scheduler>,
    pub journal: Option<Arc<JournalStore>>,
    pub handle: Handle,
    pub report_sink: ReportSink,
    /// Phase counter — incremented by `phase()` and `phase_begin()`, read by
    /// `agent()`/`parallel()` so cache keys and events carry a meaningful phase id.
    pub phase_counter: Arc<AtomicU32>,
    /// Agent sequence counter — global monotonic, incremented per `agent()` call.
    /// Shared across pipeline/parallel so every agent gets a unique display id.
    pub agent_seq_counter: Arc<AtomicU32>,
    /// Span counter — `fetch_add`'d by each blocking SDK primitive to correlate
    /// its `*Started`/`*Done` event pair (see `docs/design/sdk-events.md`).
    pub span_counter: Arc<AtomicU64>,
    /// Phase span stack — push/pop by `phase_begin()`/`phase_end()`.
    /// `phase()` reads the top as `parent_span_id`.
    pub phase_span_stack: Arc<Mutex<Vec<PhaseSpan>>>,
    /// Coroutine bridge for `pmap()` coordination.
    pub coroutine_bridge: Arc<CoroutineBridge>,
}

impl SdkContext {
    pub(crate) fn new(
        run_ctx: RunContext,
        scheduler: Arc<Scheduler>,
        report_sink: ReportSink,
        journal: Option<Arc<JournalStore>>,
        handle: Handle,
    ) -> Self {
        Self {
            run_ctx,
            scheduler,
            journal,
            handle,
            report_sink,
            phase_counter: Arc::new(AtomicU32::new(0)),
            agent_seq_counter: Arc::new(AtomicU32::new(0)),
            span_counter: Arc::new(AtomicU64::new(0)),
            phase_span_stack: Arc::new(Mutex::new(Vec::new())),
            coroutine_bridge: Arc::new(CoroutineBridge::new()),
        }
    }

    /// Convenience accessor for the run id.
    pub(crate) fn run_id(&self) -> RunId {
        self.run_ctx.run_id
    }

    /// Convenience accessor for the event sender (cloned).
    pub(crate) fn events(&self) -> EventSender {
        self.run_ctx.events.clone()
    }
}

/// A phase span entry on the span stack.
#[derive(Debug, Clone)]
pub struct PhaseSpan {
    pub id: u32,
    pub name: String,
    pub parent_id: Option<u32>,
    pub depth: u32,
    pub started_at: Instant,
    #[allow(dead_code)]
    pub planned: usize,
}

/// Internal test helpers, exposed to every sibling `#[cfg(test)] mod` under
/// `sdk::` via `crate::sdk::test_support::*`. Not part of the public API.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Build a minimal `SdkContext` backed by an empty `BackendRegistry`.
    /// Each call gets a fresh UUIDv7 run id and a 16-slot broadcast bus.
    pub(crate) fn make_sdk_context() -> SdkContext {
        use luft_core::contract::backend::RunContext;
        use luft_core::contract::ids::RunId;
        use luft_core::{BackendRegistry, Scheduler, SchedulerConfig};
        use std::sync::Mutex;
        use tokio_util::sync::CancellationToken;

        let registry = BackendRegistry::new();
        let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        scheduler.init_run_with(run_id, run_ctx.events.clone());
        let report_sink: ReportSink = Arc::new(Mutex::new(None));
        SdkContext::new(
            run_ctx,
            scheduler,
            report_sink,
            None,
            tokio::runtime::Handle::current(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use luft_core::contract::backend::{AgentTask, RunContext};
    use luft_core::contract::ids::RunId;
    use luft_core::{BackendRegistry, Scheduler, SchedulerConfig};
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    fn make_sdk_context() -> SdkContext {
        let registry = BackendRegistry::new();
        let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        scheduler.init_run_with(run_id, run_ctx.events.clone());
        let report_sink: ReportSink = Arc::new(Mutex::new(None));
        SdkContext::new(
            run_ctx,
            scheduler,
            report_sink,
            None,
            tokio::runtime::Handle::current(),
        )
    }

    fn sample_agent_task() -> AgentTask {
        AgentTask {
            agent_id: luft_core::contract::ids::AgentId::now_v7(),
            phase_id: 0,
            prompt: "p".into(),
            model: None,
            description: None,
            role: None,
            name: None,
            agent_seq: 0,
            allowlist: None,
            workdir: PathBuf::from("."),
            mcp_endpoint: None,
            timeout: None,
            output_schema: None,
        }
    }

    // -----------------------------------------------------------------------
    // CoroutineBridge — pmap coordination
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn coroutine_bridge_new_starts_outside_pmap() {
        let bridge = CoroutineBridge::new();
        assert!(!bridge.is_in_pmap());
    }

    #[tokio::test]
    async fn coroutine_bridge_enter_exit_pmap() {
        let bridge = CoroutineBridge::new();
        bridge.enter_pmap();
        assert!(bridge.is_in_pmap());
        bridge.exit_pmap();
        assert!(!bridge.is_in_pmap());
    }

    #[tokio::test]
    async fn coroutine_bridge_enter_pmap_is_idempotent() {
        let bridge = CoroutineBridge::new();
        bridge.enter_pmap();
        bridge.enter_pmap();
        bridge.enter_pmap();
        assert!(bridge.is_in_pmap());
        bridge.exit_pmap();
        assert!(!bridge.is_in_pmap());
    }

    #[tokio::test]
    async fn coroutine_bridge_deposit_returns_unique_ids() {
        let bridge = CoroutineBridge::new();
        let id1 = bridge.deposit(PendingTask {
            task: sample_agent_task(),
            backend: None,
            cache_key: luft_core::journal::AgentCacheKey::new("p", None, 0),
            agent_id: luft_core::contract::ids::AgentId::now_v7(),
            phase_id: 0,
        });
        let id2 = bridge.deposit(PendingTask {
            task: sample_agent_task(),
            backend: None,
            cache_key: luft_core::journal::AgentCacheKey::new("q", None, 0),
            agent_id: luft_core::contract::ids::AgentId::now_v7(),
            phase_id: 0,
        });
        assert_ne!(id1, id2);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }

    #[tokio::test]
    async fn coroutine_bridge_take_returns_deposited_task() {
        let bridge = CoroutineBridge::new();
        let task = sample_agent_task();
        let id = bridge.deposit(PendingTask {
            task: task.clone(),
            backend: Some("acp".into()),
            cache_key: luft_core::journal::AgentCacheKey::new("p", Some("m"), 1),
            agent_id: luft_core::contract::ids::AgentId::now_v7(),
            phase_id: 1,
        });
        let taken = bridge.take(id).expect("deposited task should exist");
        assert_eq!(taken.task.prompt, task.prompt);
        assert_eq!(taken.phase_id, 1);
        assert_eq!(taken.backend.as_deref(), Some("acp"));
    }

    #[tokio::test]
    async fn coroutine_bridge_take_is_one_shot() {
        let bridge = CoroutineBridge::new();
        let id = bridge.deposit(PendingTask {
            task: sample_agent_task(),
            backend: None,
            cache_key: luft_core::journal::AgentCacheKey::new("p", None, 0),
            agent_id: luft_core::contract::ids::AgentId::now_v7(),
            phase_id: 0,
        });
        assert!(bridge.take(id).is_some());
        assert!(bridge.take(id).is_none(), "second take must return None");
    }

    #[tokio::test]
    async fn coroutine_bridge_take_unknown_id_returns_none() {
        let bridge = CoroutineBridge::new();
        assert!(bridge.take(0).is_none());
        assert!(bridge.take(999_999).is_none());
    }

    #[tokio::test]
    async fn coroutine_bridge_concurrent_deposits_are_independent() {
        let bridge = CoroutineBridge::new();
        let mut ids = Vec::new();
        for i in 0..5 {
            ids.push(bridge.deposit(PendingTask {
                task: sample_agent_task(),
                backend: None,
                cache_key: luft_core::journal::AgentCacheKey::new("p", None, i),
                agent_id: luft_core::contract::ids::AgentId::now_v7(),
                phase_id: 0,
            }));
        }
        let unique: std::collections::HashSet<_> = ids.iter().copied().collect();
        assert_eq!(unique.len(), 5, "all deposit ids must be distinct");

        // Each id should be retrievable exactly once.
        for id in &ids {
            assert!(bridge.take(*id).is_some());
            assert!(bridge.take(*id).is_none());
        }
    }

    #[tokio::test]
    async fn coroutine_bridge_take_unknown_does_not_affect_deposited() {
        let bridge = CoroutineBridge::new();
        let real = bridge.deposit(PendingTask {
            task: sample_agent_task(),
            backend: None,
            cache_key: luft_core::journal::AgentCacheKey::new("p", None, 0),
            agent_id: luft_core::contract::ids::AgentId::now_v7(),
            phase_id: 0,
        });
        assert!(bridge.take(real + 9999).is_none());
        // The real deposit is still available.
        assert!(bridge.take(real).is_some());
    }

    // -----------------------------------------------------------------------
    // SdkContext
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn sdk_context_new_starts_with_default_counters_and_bridge() {
        let cx = make_sdk_context();
        assert_eq!(cx.phase_counter.load(Ordering::Relaxed), 0);
        assert_eq!(cx.agent_seq_counter.load(Ordering::Relaxed), 0);
        assert_eq!(cx.span_counter.load(Ordering::Relaxed), 0);
        assert!(cx.phase_span_stack.lock().unwrap().is_empty());
        assert!(!cx.coroutine_bridge.is_in_pmap());
        assert!(cx.journal.is_none());
    }

    #[tokio::test]
    async fn sdk_context_run_id_returns_underlying_run_id() {
        let registry = BackendRegistry::new();
        let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        scheduler.init_run_with(run_id, run_ctx.events.clone());
        let cx = SdkContext::new(
            run_ctx,
            scheduler,
            Arc::new(Mutex::new(None)),
            None,
            tokio::runtime::Handle::current(),
        );
        assert_eq!(cx.run_id(), run_id);
    }

    #[tokio::test]
    async fn sdk_context_events_returns_a_clone_of_the_run_ctx_sender() {
        let cx = make_sdk_context();
        let _tx = cx.events();
        // Calling twice should not consume or panic — EventSender is Clone.
        let _tx2 = cx.events();
    }

    // -----------------------------------------------------------------------
    // PendingTask
    // -----------------------------------------------------------------------
    #[test]
    fn pending_task_carries_arbitrary_field_values() {
        let task = sample_agent_task();
        let cache_key = luft_core::journal::AgentCacheKey::new("p", Some("claude"), 7);
        let pending = PendingTask {
            task: task.clone(),
            backend: Some("opencode".into()),
            cache_key: cache_key.clone(),
            agent_id: task.agent_id,
            phase_id: 7,
        };
        assert_eq!(pending.task.prompt, task.prompt);
        assert_eq!(pending.backend.as_deref(), Some("opencode"));
        assert_eq!(pending.cache_key.phase_id, 7);
        assert_eq!(pending.cache_key.model.as_deref(), Some("claude"));
        assert_eq!(pending.agent_id, task.agent_id);
        assert_eq!(pending.phase_id, 7);
    }

    // -----------------------------------------------------------------------
    // PhaseSpan — public Pubsub field shape
    // -----------------------------------------------------------------------
    #[test]
    fn phase_span_clone_preserves_fields() {
        let now = Instant::now();
        let span = PhaseSpan {
            id: 3,
            name: "explore".into(),
            parent_id: Some(2),
            depth: 1,
            started_at: now,
            planned: 4,
        };
        let cloned = span.clone();
        assert_eq!(cloned.id, 3);
        assert_eq!(cloned.name, "explore");
        assert_eq!(cloned.parent_id, Some(2));
        assert_eq!(cloned.depth, 1);
        assert_eq!(cloned.planned, 4);
        // Debug should mention key fields.
        let dbg = format!("{:?}", cloned);
        assert!(dbg.contains("PhaseSpan"));
        assert!(dbg.contains("explore"));
    }

    #[test]
    fn phase_span_with_no_parent_and_zero_planned() {
        let span = PhaseSpan {
            id: 0,
            name: String::new(),
            parent_id: None,
            depth: 0,
            started_at: Instant::now(),
            planned: 0,
        };
        assert!(span.parent_id.is_none());
        assert_eq!(span.depth, 0);
        assert_eq!(span.name, "");
        assert_eq!(span.planned, 0);
    }

    // -----------------------------------------------------------------------
    // ReportSink — compile-time sanity check on the type alias
    // -----------------------------------------------------------------------
    #[test]
    fn report_sink_can_be_constructed_and_written_to() {
        let sink: ReportSink = Arc::new(Mutex::new(None));
        {
            let mut guard = sink.lock().unwrap();
            *guard = Some(serde_json::json!({"written": true}));
        }
        let guard = sink.lock().unwrap();
        assert_eq!(guard.as_ref().unwrap(), &serde_json::json!({"written": true}));
    }

    // -----------------------------------------------------------------------
    // Public surface compile-time check
    // -----------------------------------------------------------------------
    #[test]
    fn public_api_re_exports_compile() {
        // Touch each public type so a future rename / private-pub regression
        // is caught at compile time, not at the first integration use.
        fn _touch(_: &PhaseSpan) {}
        let span = PhaseSpan {
            id: 0,
            name: "x".into(),
            parent_id: None,
            depth: 0,
            started_at: Instant::now(),
            planned: 0,
        };
        _touch(&span);
        // PendingTask is pub(crate) — used here only via super import.
        let _ = std::any::type_name::<PendingTask>();
        let _ = std::any::type_name::<SdkContext>();
        let _ = std::any::type_name::<CoroutineBridge>();
        let _ = std::any::type_name::<ReportSink>();
    }
}
