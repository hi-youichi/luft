//! Binary command handlers — one module per CLI subcommand. `main` parses
//! args and routes each subcommand to the matching handler here.

pub mod backend;
pub mod event_log;
pub mod mcp_server;
pub mod generate;
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
