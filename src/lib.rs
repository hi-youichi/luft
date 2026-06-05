//! Maestro — multi-agent orchestration runtime (v0.1).
//!
//! See [`docs/architecture.md`] for architecture overview and
//! [`docs/sdk-reference.md`] for Lua SDK primitives.
//!
//! Module layout:
//! - [`core`]      — contracts, scheduler, journal, state persistence
//! - [`runtime`]   — mlua orchestration runtime (Lua sandbox + SDK primitives)
//! - [`mcp`]       — Maestro MCP data-plane server
//! - [`adapters`]  — AcpAdapter / OpenCode backend adapters
//! - [`planner`]   — NL → Lua planner
//! - [`cli`]       — CLI + TUI + headless

pub mod core;

pub mod adapters;
pub mod cli;
pub mod mcp;
pub mod planner;
pub mod runtime;
