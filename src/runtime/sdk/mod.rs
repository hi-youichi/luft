//! SDK primitives bridging Lua orchestration scripts to the scheduler.
//!
//! Each submodule registers one logical group of SDK globals via a
//! `register_*_sdk(lua, cx)` function (the same pattern as
//! [`crate::runtime::converge::register_converge_sdk`]). All shared
//! dependencies travel through [`SdkContext`] so the individual registrars keep
//! short, uniform signatures.

pub(crate) mod agent;
pub(crate) mod control;
pub(crate) mod convert;
pub(crate) mod report;
pub(crate) mod task;
pub(crate) mod workflow;

use crate::core::contract::backend::RunContext;
use crate::core::contract::event::EventSender;
use crate::core::contract::ids::RunId;
use crate::core::journal::JournalStore;
use crate::core::Scheduler;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::runtime::Handle;

/// Report sink shared between the runtime and the `report()` primitive.
pub(crate) type ReportSink = Arc<Mutex<Option<serde_json::Value>>>;

/// Shared dependencies captured by the SDK closures during registration.
///
/// Built once per [`Runtime`](crate::runtime::Runtime) and passed by reference
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
