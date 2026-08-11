//! # luft-core
//!
//! **Frozen contracts, scheduler, journal, and state management.**
//!
//! `luft-core` is the dependency-aware foundation of the Luft ecosystem.
//! It defines the traits, types, scheduling logic, and persistence interfaces
//! that every other crate builds on, while keeping those contracts centralized.
//! Types defined here are **frozen contracts**
//! — breaking changes are treated as major version bumps.
//!
//! ## Module Overview
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | [`contract`] | Cross-crate traits and types: [`AgentBackend`], [`AgentTask`], [`AgentResult`], [`AgentEvent`], [`Finding`], [`Skill`] |
//! | [`scheduler`] | Concurrency-limited agent dispatcher with retry and journal callbacks |
//! | [`journal`] | Checkpoint store for run resume — write agent results, read on restart |
//! | [`state`] | Run/phase state machine: [`RunCheckpoint`], [`CheckpointStatus`] |
//! | [`run_dir`] | Filesystem layout helpers for `.luft/runs/<run-id>/` |
//!
//! ## Stability Guarantees
//!
//! - **Traits** ([`AgentBackend`], [`JournalCallback`]): signatures are stable
//!   within a minor version. Implementations in downstream crates are safe.
//! - **Structs** ([`AgentTask`], [`AgentResult`], [`RunCheckpoint`]): fields
//!   are `pub` and additive only (new fields require `#[serde(default)]` or a
//!   major bump).
//! - **Enums** ([`AgentStatus`], [`BackendError`], [`RunStatus`]): variants are
//!   additive — new variants may appear in minor releases.
//!
//! ## Feature Flags
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `testing` | Exports [`MockBackend`], [`MockFileBackend`], and test data generators |
//!
//! [`AgentBackend`]: contract::backend::AgentBackend
//! [`AgentTask`]: contract::backend::AgentTask
//! [`AgentResult`]: contract::backend::AgentResult
//! [`AgentEvent`]: contract::event::AgentEvent
//! [`Finding`]: contract::finding::Finding
//! [`Skill`]: contract::skill::Skill
//! [`JournalCallback`]: scheduler::JournalCallback
//! [`MockBackend`]: mock_backend::MockBackend
//! [`MockFileBackend`]: mock_file_backend::MockFileBackend

pub mod contract;
pub mod in_memory_backend;
pub mod journal;
pub mod run_dir;
pub mod scheduler;
pub mod session;
pub mod state;

pub mod json_to_lua;
pub mod mock_backend;
#[cfg(feature = "testing")]
pub mod mock_file_backend;
#[cfg(feature = "testing")]
pub mod mock_gen;
pub mod params;
pub mod query;
pub mod testing;

pub use contract::*;
pub use journal::{
    gc_runs, AgentCacheKey, CompositeJournalCallback, JournalError, JournalStore, ResumeContext,
    RunCreationMode,
};
pub use scheduler::{
    BackendRegistry, JournalCallback, RetryPolicy, Scheduler, SchedulerConfig, SchedulerError,
};
pub use state::{
    list_run_dirs, AgentResultCache, AgentSessionCheckpoint, CheckpointBackend, CheckpointStatus,
    PhaseSummary, RunCheckpoint,
};

#[cfg(feature = "testing")]
pub use mock_backend::{FailKind, MockBackend, MockBehavior};
#[cfg(feature = "testing")]
pub use mock_file_backend::{MockFileBackend, MockStats, MockStatsSnapshot};
pub use testing::{CallRecord, CountingBackend, CrashBackend, SharedBackend};
