//! # luft-storage
//!
//! **SQLite-backed structured persistence for Luft.**
//!
//! Replaces the JSONL `events.jsonl` + `checkpoint.json` pair with a queryable,
//! relational store. Provides a UI-ready query API for listing runs, inspecting
//! agent turns, and searching event spans.
//!
//! ## Module Layout
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | [`db`] | Connection pool (`DbPool`) + schema migration |
//! | [`writer`] | `AgentEvent` → SQL write path ([`EventWriter`]) |
//! | [`reader`] | UI-ready query API: [`get_run_overview`], [`get_agent_turns`], etc. |
//! | [`error`] | Unified [`StorageError`] type |
//!
//! ## Usage
//!
//! ```no_run
//! use luft_storage::{open_db, EventWriter};
//!
//! let pool = open_db("./.luft/runs/latest/luft.db")?;
//! let writer = EventWriter::new(pool.clone());
//! // writer.write_event(&event)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

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
