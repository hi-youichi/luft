//! Frozen contracts (code-design §1). These types are the shared basis for all
//! modules; once reviewed they should change rarely. A companion `CONTRACTS.md`
//! may track the freeze.

pub mod backend;
pub mod cache;
pub mod event;
pub mod finding;
pub mod ids;
pub mod schema;

// backend: AgentBackend (trait), AgentTask, AgentResult, RunContext, BackendError
pub use backend::*;
// cache: agent_cache_key (single named export; no wildcard re-export)
pub use cache::agent_cache_key;
// event: AgentEvent, EventSender, RunStatus
pub use event::*;
// finding: Finding, Severity, Location
pub use finding::*;
// ids: RunId, AgentId, PhaseId, TokenUsage
pub use ids::*;
// schema: validate_output, SchemaError
pub use schema::*;
