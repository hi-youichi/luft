//! `maestro` — multi-agent orchestration runtime.
//!
//! Re-exports all sub-crates and provides a high-level [`Maestro`] Builder API.
//!
//! See [`builder::Maestro`] for the main entry point.

pub use maestro_core as core;
pub use maestro_storage as storage;
pub use maestro_runtime as runtime;
pub use maestro_adapters as adapters;
pub use maestro_planner as planner;
pub use maestro_service as service;

#[allow(dead_code)]
mod mcp;
mod builder;
mod error;
pub mod prelude;

pub use builder::{Maestro, MaestroBuilder, RunHandle, RunOutcome};
pub use error::MaestroError;

#[cfg(feature = "testing")]
pub use maestro_core::mock_gen;
