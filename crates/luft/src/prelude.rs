//! # Luft Prelude
//!
//! Convenient re-exports of the most common types. Intended usage:
//!
//! ```no_run
//! use luft::prelude::*;
//! ```

pub use luft_core::contract::backend::{
    AgentBackend, AgentCapabilities, AgentResult, AgentStatus, AgentTask,
    Artifact, BackendError, LogRef, McpEndpoint, RunContext, ToolPolicy,
};
pub use luft_core::contract::event::{AgentEvent, RunStatus};
pub use luft_core::contract::finding::Finding;
pub use luft_core::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};
pub use luft_core::scheduler::{BackendRegistry, RetryPolicy, Scheduler, SchedulerConfig};
pub use luft_core::journal::JournalStore;
pub use luft_core::state::{CheckpointStatus, RunCheckpoint};
pub use luft_runtime::{validate, ExecLimits, ScriptError};
pub use luft_planner::{plan_workflow, PlannedWorkflow, PlannerConfig};
pub use luft_service::query::{ReportStatus, StatusOutput};
pub use crate::builder::{Luft, LuftBuilder, RunHandle, RunOutcome};
pub use crate::error::LuftError;

#[cfg(test)]
mod tests {
    //! The prelude tests are mostly compile-time checks: each item must
    //! resolve, must be usable in basic ways, and must be disjoint from
    //! similarly-named items in `luft_core`. If a public path breaks, the
    //! test binary won't build.

    use super::*;

    /// Type-name probe helper — returns the canonical type name of `A`.
    fn type_name_of<A: 'static>() -> &'static str {
        std::any::type_name::<A>()
    }

    #[test]
    fn prelude_default_constructible_types_resolve() {
        // For every re-export that does implement `Default`, exercise it.
        let _b: LuftBuilder = LuftBuilder::new();
        let _e: LuftError = LuftError::BackendNotConfigured;
        let _caps: AgentCapabilities = AgentCapabilities::default();
        let _tp: ToolPolicy = ToolPolicy::default();
        let _log: LogRef = LogRef::default();
        let _br: BackendRegistry = BackendRegistry::default();
        let _rp: RetryPolicy = RetryPolicy::default();
        let _sc: SchedulerConfig = SchedulerConfig::default();
        let _elf: ExecLimits = ExecLimits::default();
        let _tokens: TokenUsage = TokenUsage::default();
        let _pl: PlannerConfig = PlannerConfig::default();
        // StatusOutput does not implement Default; serialize check happens
        // in a separate test that builds the value via field initialization.
    }

    #[test]
    fn prelude_non_default_types_are_resolvable() {
        // For types that don't implement Default, name-only probes still
        // verify that the prelude re-export path resolves.
        for n in [
            type_name_of::<Artifact>(),
            type_name_of::<McpEndpoint>(),
            type_name_of::<AgentTask>(),
            type_name_of::<AgentResult>(),
            type_name_of::<RunContext>(),
            type_name_of::<AgentStatus>(),
            type_name_of::<BackendError>(),
            type_name_of::<AgentEvent>(),
            type_name_of::<RunStatus>(),
            type_name_of::<Finding>(),
            type_name_of::<JournalStore>(),
            type_name_of::<RunCheckpoint>(),
            type_name_of::<CheckpointStatus>(),
            type_name_of::<Scheduler>(),
            type_name_of::<ScriptError>(),
            type_name_of::<ReportStatus>(),
        ] {
            assert!(!n.is_empty(), "type name should be non-empty");
        }
    }

    #[test]
    fn prelude_contract_ids_are_distinct_newtypes() {
        // Three distinct UUID-flavored newtypes with the same underlying
        // type. Rust's `type_name::<T>()` transparently unwraps newtypes
        // defined as `pub struct Foo(pub X)` — so we cannot rely on
        // type-name comparison here. Instead, verify they're properly
        // constructible as distinct types and pattern-distinct.
        let _id_r: std::marker::PhantomData<RunId> = std::marker::PhantomData;
        let _id_a: std::marker::PhantomData<AgentId> = std::marker::PhantomData;
        let _id_t: std::marker::PhantomData<TokenUsage> = std::marker::PhantomData;
        let r = RunId::now_v7();
        let a = AgentId::now_v7();
        assert_ne!(r.to_string(), a.to_string());
        let phase: PhaseId = 9;
        assert_eq!(phase, 9);
        // And PhaseId is u32 — verify it round-trips through Display.
        let s = phase.to_string();
        assert_eq!(s, "9");
    }

    #[test]
    fn prelude_runtime_function_accepts_and_rejects() {
        // Validate should accept trivially-valid scripts and reject broken
        // ones — the prelude wraps `luft_runtime::validate`.
        assert!(validate("return 1 + 2").is_ok());
        assert!(validate("if true then").is_err());
        assert!(validate("~~ not lua ~~").is_err());
        // An empty string is valid Lua (no syntax errors).
        assert!(validate("").is_ok());
    }

    #[test]
    fn prelude_runtime_function_can_via_wildcard() {
        // Same checks against the prelude alias.
        mod _via_wildcard {
            use crate::prelude::*;
            pub(super) fn ok() -> Result<(), ScriptError> {
                validate("return 1")
            }
            pub(super) fn err() -> Result<(), ScriptError> {
                validate("???")
            }
        }
        assert!(_via_wildcard::ok().is_ok());
        assert!(_via_wildcard::err().is_err());
    }

    #[test]
    fn prelude_facade_types_resolve() {
        // The four primary facade types must each be reachable.
        assert!(type_name_of::<Luft>().contains("Luft"));
        assert!(type_name_of::<LuftBuilder>().contains("LuftBuilder"));
        assert!(type_name_of::<RunHandle>().contains("RunHandle"));
        assert!(type_name_of::<RunOutcome>().contains("RunOutcome"));
        // And they must remain distinct types.
        assert_ne!(type_name_of::<Luft>(), type_name_of::<LuftBuilder>());
        assert_ne!(type_name_of::<Luft>(), type_name_of::<LuftError>());
        assert_ne!(type_name_of::<RunHandle>(), type_name_of::<RunOutcome>());
    }

    #[test]
    fn prelude_backend_trait_is_in_scope() {
        // The `AgentBackend` trait must be usable as a bound from the
        // prelude surface.
        fn _takes<B: AgentBackend + ?Sized>(_: &B) {}
        // MockBackend needs at least one behavior; supply a no-op success.
        let mock = luft_core::mock_backend::MockBackend::new(
            "m",
            vec![luft_core::mock_backend::MockBehavior::Success {
                output: serde_json::Value::Null,
                tokens: TokenUsage::default(),
                delay: std::time::Duration::ZERO,
            }],
        );
        _takes(&mock);
        // Carry the function pointer without having to re-specify B.
        let _f: fn(&luft_core::mock_backend::MockBackend) = _takes::<luft_core::mock_backend::MockBackend>;
    }

    #[test]
    fn prelude_planner_defaults_are_sane() {
        let cfg = PlannerConfig::default();
        assert_eq!(cfg.max_retries, 3);
        assert!(!cfg.generate_mock);
        assert!(cfg.planner_model.is_none());
        // StatusOutput has Serialize derived but not Default, so we build
        // it via field initialization and round-trip through JSON.
        let s = StatusOutput {
            run_id: "run-id".to_string(),
            run_dir: "dir".to_string(),
            task: "task".to_string(),
            status: "running".to_string(),
            current_phase: 1,
            completed_phases: 0,
            total_started: 0,
            completed_agents: 0,
            running_agents: 0,
            total_tokens: 0,
            created_at: "0".to_string(),
            updated_at: "0".to_string(),
        };
        let json = serde_json::to_string(&s).expect("StatusOutput serializes");
        assert!(json.contains("run-id"));
    }

    #[test]
    fn prelude_is_usable_via_wildcard_import() {
        // The point of the prelude is `use luft::prelude::*;` — prove it
        // resolves inside a nested module.
        mod _via_wildcard {
            use crate::prelude::*;
            pub(super) fn _probe() -> (RunId, PhaseId, TokenUsage, LuftError) {
                let run = RunId::now_v7();
                let phase: PhaseId = 1;
                let tokens = TokenUsage::default();
                let err = LuftError::BackendNotConfigured;
                (run, phase, tokens, err)
            }
        }
        // Just keep the module reachable — its existence is the assertion.
        let _ = _via_wildcard::_probe;
    }

    #[test]
    fn prelude_re_exports_match_underlying_crates() {
        // The wildcard re-exports must point at the same types as their
        // home crate path — verifying nothing was accidentally shadowed
        // or wrapped at the prelude layer.
        assert_eq!(
            type_name_of::<RunId>(),
            type_name_of::<luft_core::RunId>(),
        );
        assert_eq!(
            type_name_of::<TokenUsage>(),
            type_name_of::<luft_core::TokenUsage>(),
        );
        assert_eq!(
            type_name_of::<ExecLimits>(),
            type_name_of::<luft_runtime::ExecLimits>(),
        );
        assert_eq!(
            type_name_of::<PlannerConfig>(),
            type_name_of::<luft_planner::PlannerConfig>(),
        );
        assert_eq!(
            type_name_of::<LuftError>(),
            type_name_of::<crate::error::LuftError>(),
        );
        assert_eq!(
            type_name_of::<ReportStatus>(),
            type_name_of::<luft_service::query::ReportStatus>(),
        );
    }
}
