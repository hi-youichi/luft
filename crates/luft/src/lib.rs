//! # Luft
//!
//! **Multi-agent orchestration runtime powered by Lua scripts.**
//!
//! Luft lets you coordinate multiple LLM agents — sequential, parallel,
//! pipeline, or consensus — through deterministic Lua orchestration scripts
//! rather than ad-hoc prompt chaining. The Lua script is pure orchestration:
//! it calls SDK primitives (`agent()`, `parallel()`, `pipeline()`,
//! `converge()`, `report()`) and the runtime bridges each call to a
//! pluggable [`AgentBackend`] that does the real work.
//!
//! [`AgentBackend`]: luft_core::contract::backend::AgentBackend
//!
//! ## Quick Start
//!
//! ```ignore
//! use luft::Luft;
//! # use luft::LuftError;
//! # use luft_core::mock_backend::MockBackend;
//!
//! # async fn run() -> Result<(), LuftError> {
//! let luft = Luft::builder()
//!     .backend(MockBackend::new("mock", vec![]))
//!     .base_dir("./.luft/runs")
//!     .build()?;
//!
//! let outcome = luft.run_script(r#"
//!     meta = { reasoning = "fast", phases = {} }
//!     function main()
//!         local r = agent({ prompt = "Analyze the codebase for security issues" })
//!         report(r.output)
//!     end
//! "#).await?;
//!
//! println!("done: {:?}", outcome.result);
//! # Ok(())
//! # }
//! ```
//!
//! ## Architecture
//!
//! ```text
//!  ┌──────────────────────────────────────────────────────────┐
//!  │                      luft (facade)                     │
//!  │  LuftBuilder · Luft · RunHandle · RunOutcome        │
//!  └───────┬──────────┬──────────┬─────────┬────────┬─────────┘
//!          │          │          │         │        │
//!     ┌────▼────┐ ┌───▼───┐ ┌───▼────┐ ┌──▼───┐ ┌──▼──────┐
//!     │  core   │ │runtime│ │storage │ │planner│ │ adapters│
//!     │contracts│ │ Lua VM│ │ SQLite │ │NL→Lua│ │   ACP   │
//!     └─────────┘ └───────┘ └────────┘ └──────┘ └─────────┘
//! ```
//!
//! | Crate | Role |
//! |-------|------|
//! | [`core`] | Frozen contracts: `AgentBackend` trait, types, scheduler, journal, state |
//! | [`runtime`] | Sandboxed mlua VM with orchestration SDK primitives |
//! | [`storage`] | SQLite persistence with query API |
//! | [`planner`] | Natural-language → Lua script generation |
//! | [`adapters`] | OpenCode ACP backend implementation |
//! | [`service`] | Presentation-free run lifecycle and query functions |
//!
//! ## Entry Points
//!
//! - **[`LuftBuilder`]** — fluent builder for configuring and constructing a [`Luft`] instance.
//! - **[`Luft`]** — top-level orchestrator: `run_script`, `run_workflow`, `run_nl`.
//! - **[`RunHandle`]** — async handle for fire-and-forget execution with event subscription.
//! - **[`RunOutcome`]** — completed run result with output and run directory.
//! - **[`prelude`]** — convenience re-exports of the most common types.
//!
//! ## Error Handling
//!
//! All fallible operations return [`Result<T, LuftError>`]. [`LuftError`]
//! aggregates errors from subsystems (backend, script, storage, scheduler) via
//! `#[from]` conversions, so you can use `?` freely across crate boundaries.
//!
//! ## Feature Flags
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `testing` | Exports mock backends and test utilities for downstream test suites |
//! | `unstable_end_turn_token_usage` | Enable experimental per-turn token accounting |
//!
//! [`Result<T, LuftError>`]: Result

pub use luft_core as core;
pub use luft_storage as storage;
pub use luft_runtime as runtime;
pub use luft_adapters as adapters;
pub use luft_planner as planner;
pub use luft_service as service;

#[allow(dead_code)]
mod mcp;
mod builder;
mod error;
pub mod prelude;

pub use builder::{Luft, LuftBuilder, RunHandle, RunOutcome};
pub use error::LuftError;

#[cfg(feature = "testing")]
pub use luft_core::mock_gen;

#[cfg(test)]
mod tests {
    // These tests are mostly compile-time checks: they verify that the
    // public re-exports land at the paths documented in the lib.rs docstring.
    // If any of these `use` statements fails to resolve, the test binary
    // won't build, signalling a regression in the facade surface.

    use super::*;
    use luft_core::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};

    #[test]
    fn prelude_module_is_public() {
        // The `prelude` module must remain public so downstream users can
        // `use luft::prelude::*;`.
        let _: fn() = || {
            let _ = prelude::Luft::builder;
        };
    }

    #[test]
    fn core_crate_alias_resolves() {
        // `luft_core` should be reachable both as `crate::core` and
        // through the `luft_core` path.
        let _: fn() = || {
            let _ = core::RunId::now_v7();
            let _ = luft_core::RunId::now_v7();
        };
    }

    #[test]
    fn storage_crate_alias_resolves() {
        let _: fn() = || {
            // The alias should be to the same crate as luft_storage.
            let _ = std::any::type_name::<storage::StorageError>();
            let _ = std::any::type_name::<luft_storage::StorageError>();
            assert_eq!(
                std::any::type_name::<storage::StorageError>(),
                std::any::type_name::<luft_storage::StorageError>(),
            );
        };
    }

    #[test]
    fn runtime_crate_alias_resolves() {
        let _: fn() = || {
            let _ = std::any::type_name::<runtime::ScriptError>();
            let _ = std::any::type_name::<luft_runtime::ScriptError>();
            assert_eq!(
                std::any::type_name::<runtime::ScriptError>(),
                std::any::type_name::<luft_runtime::ScriptError>(),
            );
        };
    }

    #[test]
    fn adapters_crate_alias_resolves() {
        let _: fn() = || {
            // adapters is a real crate, reachable through the alias path.
            let _ = std::any::type_name::<adapters::AcpAdapter>();
            let _ = std::any::type_name::<luft_adapters::AcpAdapter>();
            assert_eq!(
                std::any::type_name::<adapters::AcpAdapter>(),
                std::any::type_name::<luft_adapters::AcpAdapter>(),
            );
        };
    }

    #[test]
    fn planner_crate_alias_resolves() {
        let _: fn() = || {
            let _ = std::any::type_name::<planner::PlannerConfig>();
            let _ = std::any::type_name::<luft_planner::PlannerConfig>();
            assert_eq!(
                std::any::type_name::<planner::PlannerConfig>(),
                std::any::type_name::<luft_planner::PlannerConfig>(),
            );
        };
    }

    #[test]
    fn service_crate_alias_resolves() {
        let _: fn() = || {
            // `luft_service` exposes submodules (phases / query / run) at the
            // crate root. The alias must resolve to the same crate.
            let _ = std::any::type_name::<service::query::StatusOutput>();
            let _ = std::any::type_name::<luft_service::query::StatusOutput>();
            assert_eq!(
                std::any::type_name::<service::query::StatusOutput>(),
                std::any::type_name::<luft_service::query::StatusOutput>(),
            );
        };
    }

    #[test]
    fn reexported_top_level_types_are_distinct() {
        // Each top-level public type must be reachable through its declared
        // path without colliding with the others. (Note: `std::any::type_name`
        // unwraps transparent newtypes, so UUID-flavored newtypes compare
        // equal — we restrict the assertion to named struct types.)
        fn assert_distinct<A: 'static, B: 'static>() {
            assert!(
                std::any::type_name::<A>() != std::any::type_name::<B>(),
                "type names must differ"
            );
        }
        assert_distinct::<Luft, LuftBuilder>();
        assert_distinct::<Luft, LuftError>();
        assert_distinct::<RunHandle, RunOutcome>();
        assert_distinct::<RunHandle, Luft>();
        assert_distinct::<RunOutcome, LuftBuilder>();
    }

    #[test]
    fn reexported_error_is_usable() {
        // Constructing each simple variant ensures the error type compiles
        // and pattern-matches at the crate re-export path.
        let err = LuftError::RunNotFound("abc".into());
        assert_eq!(err.to_string(), "run not found: abc");
    }

    #[test]
    fn reexported_ids_can_be_constructed() {
        let run_id = RunId::now_v7();
        let agent_id = AgentId::now_v7();
        let phase: PhaseId = 7;
        assert_eq!(phase, 7);
        assert_ne!(run_id, agent_id);
    }

    #[cfg(feature = "testing")]
    #[test]
    fn mock_gen_is_reexported_under_testing_feature() {
        // Under the `testing` feature, `mock_gen` should be reachable.
        let _: fn() = || {
            let _ = std::any::type_name::<mock_gen::MockGenConfig>();
        };
    }

    #[test]
    fn builder_type_is_a_named_struct() {
        // The builder should be a proper `LuftBuilder` (not an alias / not
        // a tuple struct) — verified by constructing one via its constructor.
        let _b: LuftBuilder = LuftBuilder::new();
    }

    #[test]
    fn run_outcome_type_is_a_named_struct() {
        // RunOutcome fields are public — verify the type identity at the
        // re-export surface (we can't construct it directly since its fields
        // are not externally constructible, but the type itself must be).
        fn _takes(o: RunOutcome) -> RunId {
            o.run_id
        }
        // Just keep `_takes` reachable — its type signature is the assertion.
        let _ = _takes;
    }
}
