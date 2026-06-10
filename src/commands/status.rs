//! `status` subcommand: show the checkpoint summary of a past run.

use super::runs_base_dir;
use anyhow::Result;

pub fn status_run_cmd(run_id: uuid::Uuid) -> Result<()> {
    let base_dir = runs_base_dir();
    // Data access (existence-checked) lives in the query layer; this command
    // owns only the presentation below.
    let checkpoint = maestro::service::query::get_checkpoint(run_id, &base_dir)?
        .ok_or_else(|| anyhow::anyhow!("run not found or has no checkpoint: {}", run_id))?;

    println!("=== Run Status ===");
    println!("  Run ID:        {}", checkpoint.run_id);
    println!("  Task:          {}", checkpoint.task);
    println!("  Status:        {:?}", checkpoint.status);
    println!("  Current phase: {}", checkpoint.current_phase);
    println!("  Total tokens:  {}", checkpoint.total_tokens);
    println!("  Created:       {}", checkpoint.created_at);
    println!("  Updated:       {}", checkpoint.updated_at);

    if !checkpoint.completed_phases.is_empty() {
        println!("\n  Completed phases:");
        for phase in &checkpoint.completed_phases {
            println!("    - Phase {}: {} (ok={}, failed={})",
                phase.phase_id, phase.label, phase.ok, phase.failed);
        }
    }

    let agent_count = checkpoint.agent_results.len();
    if agent_count > 0 {
        println!("\n  Agent results: {} agents", agent_count);
    }

    let findings_count = checkpoint.findings.len();
    if findings_count > 0 {
        println!("  Findings: {} total", findings_count);
    }

    Ok(())
}
