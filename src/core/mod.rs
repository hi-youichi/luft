//! `core` — frozen contracts (§1) + scheduler (M1) + state (M5).
//!
//! `core` has no upstream dependency: it only defines the traits, types,
//! scheduling logic and (later) persistence that every other module builds on.

pub mod contract;
pub mod journal;
pub mod mock_backend;
pub mod run_dir;
pub mod scheduler;
pub mod state;

pub use contract::*;
pub use journal::{
    gc_runs, AgentCacheKey, CompositeJournalCallback, JournalError, JournalStore, ResumeContext,
    RunCreationMode,
};
pub use mock_backend::{FailKind, MockBackend, MockBehavior};
pub use scheduler::{
    BackendRegistry, JournalCallback, RetryPolicy, Scheduler, SchedulerConfig, SchedulerError,
};
pub use state::{
    get_run_store, list_runs, AgentResultCache, CheckpointStatus, PhaseSummary, RunCheckpoint,
    RunStore,
};
