//! Scheduler errors (§2.2).

use crate::core::contract::backend::BackendError;

#[derive(thiserror::Error, Debug)]
pub enum SchedulerError {
    #[error("unknown backend: {0}")]
    UnknownBackend(String),
    #[error("no backend registered")]
    NoBackendRegistered,
    #[error("run not initialized: {0}")]
    RunNotFound(crate::core::contract::RunId),
    #[error("quota exceeded: limit={limit}, used={used}")]
    QuotaExceeded { limit: u32, used: u32 },
    #[error("run cancelled")]
    RunCancelled,
    #[error("agent cancelled")]
    AgentCancelled,
    #[error("backend error (non-retryable): {0}")]
    NonRetryable(#[from] BackendError),
    #[error("backend error after {attempts} attempts: {source}")]
    Exhausted { attempts: u32, source: BackendError },
    #[error("output schema validation failed: {0}")]
    SchemaValidation(String),
}
