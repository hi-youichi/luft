use maestro_core::contract::backend::BackendError;
use maestro_runtime::ScriptError;
use maestro_storage::StorageError;
use maestro_core::scheduler::SchedulerError;

/// Unified error type for all Maestro operations.
///
/// Each variant wraps the error type of a subsystem. `#[from]` conversions
/// allow `?` to propagate errors across crate boundaries without manual mapping.
#[derive(thiserror::Error, Debug)]
pub enum MaestroError {
    #[error(transparent)]
    Backend(#[from] BackendError),

    #[error(transparent)]
    Script(#[from] ScriptError),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Scheduler(#[from] SchedulerError),

    #[error("run not found: {0}")]
    RunNotFound(String),

    #[error("run not resumable: {0}")]
    NotResumable(String),

    #[error("backend not configured")]
    BackendNotConfigured,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
