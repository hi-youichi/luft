//! Binary command handlers — one module per CLI subcommand. `main` parses
//! args and routes each subcommand to the matching handler here.

pub mod lua_validate;
pub mod backend;
pub mod artifact_writer;
pub mod event_log;
pub mod phase_renderer;
pub mod mcp_server;
pub mod generate;
pub mod clear;
pub mod list;
pub mod logs;
pub mod run;
pub mod save;
pub mod status;
pub mod workflows;

/// Runs are stored in `.maestro/runs` relative to the current working directory.
pub(crate) fn runs_base_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(".")
        .join(".maestro")
        .join("runs")
}

/// Shared global CWD lock for tests that change the working directory.
/// Individual test modules (list, status, logs) MUST use this instead of
/// their own local mutex to prevent cross-module CWD races.
#[cfg(test)]
pub(crate) static GLOBAL_CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
