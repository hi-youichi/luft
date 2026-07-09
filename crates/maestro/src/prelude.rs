//! Prelude: convenient re-exports for the most common Maestro types.

pub use maestro_core::contract::backend::{
    AgentBackend, AgentCapabilities, AgentResult, AgentStatus, AgentTask,
    Artifact, BackendError, LogRef, McpEndpoint, RunContext, ToolPolicy,
};
pub use maestro_core::contract::event::{AgentEvent, RunStatus};
pub use maestro_core::contract::finding::Finding;
pub use maestro_core::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};
pub use maestro_core::scheduler::{BackendRegistry, RetryPolicy, Scheduler, SchedulerConfig};
pub use maestro_core::journal::JournalStore;
pub use maestro_core::state::{CheckpointStatus, RunCheckpoint};
pub use maestro_runtime::{validate, ExecLimits, ScriptError};
pub use maestro_planner::{plan_workflow, PlannedWorkflow, PlannerConfig};
pub use maestro_service::query::{ReportStatus, StatusOutput};
pub use crate::builder::{Maestro, MaestroBuilder, RunHandle, RunOutcome};
pub use crate::error::MaestroError;
