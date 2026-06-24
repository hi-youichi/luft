//! SQLite-backed structured persistence for Maestro.
//!
//! Replaces the JSONL events.jsonl + checkpoint.json pair with a queryable,
//! relational store. See `docs/design/sqlite-persistence.md` for design.
//!
//! Module layout:
//! - [`db`]     — connection pool + schema migration
//! - [`writer`] — `AgentEvent` → SQL write path
//! - [`reader`] — UI-ready query API
//! - [`error`]  — unified error type

pub mod db;
pub mod error;
pub mod reader;
pub mod writer;

pub use db::{open_db, DbPool, DEFAULT_DB_PATH};
pub use error::StorageError;
pub use reader::{
    get_agent_overview, get_agent_turns, get_run_agents, get_run_overview, get_run_spans,
    get_run_tree, list_runs, search_turns, AgentOverview, RunOverview, RunSummary, SpanRow,
    TurnKindCount, TurnRow,
};
pub use writer::EventWriter;