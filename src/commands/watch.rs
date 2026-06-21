//! `watch` subcommand: open a TUI replay of a past or running run.
//!
//! Usage:
//!   `maestro watch <run_dir>` — replay events from `.maestro/runs/<run_dir>/events.jsonl`

use crate::commands::runs_base_dir;
use anyhow::Result;

pub async fn watch_run(run_dir: String) -> Result<()> {
    let base_dir = runs_base_dir();
    let dir = base_dir.join(&run_dir);
    if !dir.exists() {
        anyhow::bail!("run directory not found: {}", dir.display());
    }
    maestro::tui::run_replay(&run_dir, &base_dir).await
}
