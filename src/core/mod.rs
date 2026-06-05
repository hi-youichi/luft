//! `core` — frozen contracts (§1) + scheduler (M1) + state (M5).
//!
//! `core` has no upstream dependency: it only defines the traits, types,
//! scheduling logic and (later) persistence that every other module builds on.

pub mod contract;
pub mod journal;
pub mod mock_backend;
pub mod scheduler;
pub mod state;

pub use contract::*;
pub use journal::{
    AgentCacheKey, JournalError, JournalStore,
    ResumeContext, RunCreationMode, CompositeJournalCallback, gc_runs,
};
pub use scheduler::{BackendRegistry, JournalCallback, RetryPolicy, Scheduler, SchedulerConfig, SchedulerError};
pub use mock_backend::{MockBackend, MockBehavior, FailKind};
pub use state::{
    RunCheckpoint, RunStore, CheckpointStatus, PhaseSummary, AgentResultCache,
    get_run_store, list_runs,
};
