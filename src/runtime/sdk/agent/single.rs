//! `agent(opts)` — run a single agent through the scheduler.
//!
//! On a journal cache hit it emits a resume log and returns the cached result
//! without re-running; otherwise it blocks on the scheduler and records the
//! outcome back into the journal.

use super::journal::{record, slot_from_cache, slot_from_result};
use crate::core::contract::event::{AgentEvent, LogLevel};
use crate::runtime::sdk::task::{build_result_table, build_task};
use crate::runtime::sdk::SdkContext;
use mlua::{Lua, Table};
use std::sync::atomic::Ordering;

/// Register `agent` as a Lua global.
pub(super) fn register(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    let globals = lua.globals();
    let run_id = cx.run_id();
    let sched = cx.scheduler.clone();
    let journal = cx.journal.clone();
    let handle = cx.handle.clone();
    let events = cx.events();
    let phase_counter = cx.phase_counter.clone();
    let agent_seq_counter = cx.agent_seq_counter.clone();

    let agent_fn = lua.create_function(move |lua, opts: Table| {
        let phase_id = phase_counter.load(Ordering::Relaxed);
        let (task, cache_key, backend) = build_task(&opts, phase_id, &agent_seq_counter)?;

        // M1 resume: skip already-completed agents.
        if let Some(ref j) = journal {
            if let Some(cached) = j.get_cached(&cache_key) {
                let _ = events.send(AgentEvent::Log {
                    run_id,
                    agent_id: None,
                    level: LogLevel::Info,
                    msg: format!(
                        "resume: skip cached agent ({}…)",
                        &cache_key.hash[..8.min(cache_key.hash.len())]
                    ),
                });
                let (status, output, tokens, findings) = slot_from_cache(cached);
                return build_result_table(lua, &status, output, tokens, &findings);
            }
        }

        let agent_id = task.agent_id;
        tracing::debug!(%agent_id, backend = ?backend, "agent() submitting to scheduler");
        let result = handle
            .block_on(sched.run_agent(run_id, task, backend.as_deref()))
            .map_err(|e| {
                tracing::error!(%agent_id, error = %e, "agent() scheduler error");
                mlua::Error::RuntimeError(format!("agent error: {}", e))
            })?;

        tracing::debug!(%agent_id, "agent() completed");
        record(&journal, &cache_key, agent_id, phase_id, &result);

        let (status, output, tokens, findings) = slot_from_result(result);
        build_result_table(lua, &status, output, tokens, &findings)
    })?;
    globals.set("agent", agent_fn)?;
    Ok(())
}
