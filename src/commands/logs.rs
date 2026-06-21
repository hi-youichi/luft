//! `logs` subcommand: dump the (most recent N) event log of a past run.

use super::runs_base_dir;
use anyhow::Result;

pub fn logs_run_cmd(run_dir: String, limit: Option<usize>) -> Result<()> {
    let base_dir = runs_base_dir();
    if !base_dir.join(&run_dir).exists() {
        anyhow::bail!("run not found: {}", run_dir);
    }
    let events = maestro::service::query::get_events(&run_dir, &base_dir)?;
    if events.is_empty() {
        println!("No events for run {}", run_dir);
        return Ok(());
    }

    let limit = limit.unwrap_or(100);
    let events: Vec<_> = events.into_iter().rev().take(limit).rev().collect();

    for event in events {
        let json = serde_json::to_string(&event)?;
        println!("{}", json);
    }

    Ok(())
}
