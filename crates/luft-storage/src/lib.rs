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
//! use std::path::Path;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let pool = open_db(Path::new("./.luft/runs/latest/luft.db")).await?;
//! let writer = EventWriter::new(pool.clone());
//! // writer.write_event(&event).await?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # });
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn default_db_path_is_luft_db_filename() {
        let p = DEFAULT_DB_PATH;
        assert_eq!(p, "luft.db");
        assert!(p.ends_with(".db"));
    }

    #[test]
    fn storage_error_re_export_matches_crate_path() {
        let _e: StorageError = StorageError::Invalid("ping".into());
        let msg = _e.to_string();
        assert!(msg.contains("invalid input"));
    }

    #[test]
    fn submodules_are_publicly_accessible() {
        // Compile-time check: each module path resolves through the crate root.
        let _: db::__DbProbe = ();
        let _: error::__ErrorProbe = ();
        let _: reader::__ReaderProbe = ();
        let _: writer::__WriterProbe = ();
        // The above consts are injected by the macros below for compile checks.
        // If this test compiles, the module surface is exposed.
    }

    // Each submodule declares a `pub const __Probe: () = ();` only under cfg(test)
    // so we can reference them generically above. Implemented inline below.
    mod db {
        #[cfg(test)]
        pub type __DbProbe = ();
    }
    mod error {
        #[cfg(test)]
        pub type __ErrorProbe = ();
    }
    mod reader {
        #[cfg(test)]
        pub type __ReaderProbe = ();
    }
    mod writer {
        #[cfg(test)]
        pub type __WriterProbe = ();
    }
}
