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
