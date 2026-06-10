//! Binary command handlers — one module per CLI subcommand. `main` parses
//! args and routes each subcommand to the matching handler here.

pub mod list;
pub mod logs;
pub mod run;
pub mod save;
pub mod serve;
pub mod status;
pub mod workflows;

/// Runs are stored in `.maestro/runs` relative to the current working directory.
pub(crate) fn runs_base_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(".")
        .join(".maestro")
        .join("runs")
}
