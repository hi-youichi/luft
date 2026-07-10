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

#[cfg(test)]
mod tests {
    //! The `contract` module is the frozen public API surface of `luft-core`.
    //! These tests are *compile-time* checks: they construct every re-exported
    //! item so any future drift (e.g. accidentally removing a wildcard export,
    //! renaming a type without updating this module) breaks the build before
    //! it reaches downstream crates.

    use super::*;

    #[test]
    fn reexports_are_accessible() {
        // backend
        let _: AgentCapabilities = AgentCapabilities::default();
        let _: AgentStatus = AgentStatus::Ok;
        let _: AgentTask;
        let _: AgentResult;
        let _: RunContext;
        let _: BackendError = BackendError::Cancelled;
        let _: ToolPolicy = ToolPolicy::default();
        let _: McpEndpoint;
        let _: Artifact;
        let _: LogRef = LogRef::default();

        // cache
        let _: fn(&str, Option<&str>, &str, u32) -> String = agent_cache_key;

        // event
        let _: EventSender;
        let _: AgentEvent;
        let _: RunStatus = RunStatus::Completed;
        let _: LogLevel = LogLevel::Info;
        let _: PlanPhase;
        let _: ProgressDelta;

        // finding
        let _: Finding;
        let _: Severity = Severity::Info;
        let _: Location;

        // ids
        let _: RunId = uuid::Uuid::nil();
        let _: AgentId = uuid::Uuid::nil();
        let _: PhaseId = 0u32;
        let _: TokenUsage = TokenUsage::default();

        // schema
        let _: fn(&serde_json::Value, &serde_json::Value) -> Result<(), SchemaError> =
            validate_output;
    }

    #[test]
    fn submodule_paths_resolve() {
        // Direct submodule paths must still resolve even though we also
        // re-export with wildcards. This catches "moved submodule" breakage.
        fn _assert_trait(_: &dyn backend::AgentBackend) {}
        let _: event::AgentEvent;
        let _: finding::Finding;
        let _: ids::TokenUsage;
        let _: schema::SchemaError;
        // cache module exports the `agent_cache_key` function via its own path.
        let _: fn(&str, Option<&str>, &str, u32) -> String = cache::agent_cache_key;
    }

    #[test]
    fn backend_error_is_retryable_helper() {
        assert!(BackendError::Timeout.is_retryable());
        assert!(BackendError::Spawn("x".into()).is_retryable());
        assert!(!BackendError::Cancelled.is_retryable());
        assert!(!BackendError::Protocol("x".into()).is_retryable());
        assert!(!BackendError::Config("x".into()).is_retryable());
    }

    #[test]
    fn agent_status_as_str_is_consistent() {
        // Spot-check that re-exported `AgentStatus` retains its `as_str()`
        // helper exposed by the backend submodule.
        assert_eq!(AgentStatus::Ok.as_str(), "ok");
        assert_eq!(AgentStatus::TimedOut.as_str(), "timed_out");
    }

    #[test]
    fn token_usage_default_is_zero() {
        let t = TokenUsage::default();
        assert_eq!(t.total(), 0);
    }
}
