//! `maestro-core` — frozen contracts + scheduler + journal + state.
//!
//! Core has no internal dependency: it only defines the traits, types,
//! scheduling logic and persistence that every other module builds on.

pub mod contract;
pub mod journal;
pub mod run_dir;
pub mod scheduler;
pub mod state;

#[cfg(feature = "testing")]
pub mod mock_backend;
#[cfg(feature = "testing")]
pub mod mock_file_backend;
#[cfg(feature = "testing")]
pub mod mock_gen;

pub use contract::*;
pub use journal::{
    gc_runs, AgentCacheKey, CompositeJournalCallback, JournalError, JournalStore, ResumeContext,
    RunCreationMode,
};
pub use scheduler::{
    BackendRegistry, JournalCallback, RetryPolicy, Scheduler, SchedulerConfig, SchedulerError,
};
pub use state::{
    get_run_store, list_runs, AgentResultCache, CheckpointStatus, PhaseSummary, RunCheckpoint,
    RunStore,
};

#[cfg(feature = "testing")]
pub use mock_backend::{FailKind, MockBackend, MockBehavior};
#[cfg(feature = "testing")]
pub use mock_file_backend::{MockFileBackend, MockStats, MockStatsSnapshot};
