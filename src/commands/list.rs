//! `list` subcommand: list past runs (most recent first).

use super::runs_base_dir;
use anyhow::Result;

pub fn list_runs_cmd(limit: Option<usize>) -> Result<()> {
    // Presentation only: the service layer loads + sorts (newest first) and
    // skips runs without a checkpoint.
    let base_dir = runs_base_dir();
    let runs = maestro::service::query::list_runs(&base_dir)?;
    if runs.is_empty() {
        println!("No runs found.");
        return Ok(());
    }

    let total = runs.len();
    let limit = limit.unwrap_or(20);
    let shown: Vec<_> = runs.into_iter().take(limit).collect();

    println!("Past runs ({} total, showing {}):", total, shown.len());
    for run in shown {
        println!("  {}  [{}]", run.run_dir, run.status);
    }

    Ok(())
}
