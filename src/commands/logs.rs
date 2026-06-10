//! `logs` subcommand: dump the (most recent N) event log of a past run.

use super::runs_base_dir;
use anyhow::Result;

pub fn logs_run_cmd(run_id: uuid::Uuid, limit: Option<usize>) -> Result<()> {
    let base_dir = runs_base_dir();
    // Guard against unknown ids (the store would otherwise create the dir);
    // data access lives in the query layer, slicing/formatting stays here.
    if !base_dir.join(run_id.to_string()).exists() {
        anyhow::bail!("run not found: {}", run_id);
    }
    let events = maestro::service::query::get_events(run_id, &base_dir)?;
    if events.is_empty() {
        println!("No events for run {}", run_id);
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
