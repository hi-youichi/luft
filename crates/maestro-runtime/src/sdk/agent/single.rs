//! `agent(opts)` — run a single agent through the scheduler.
//!
//! On a journal cache hit it emits a resume log and returns the cached result
//! without re-running; otherwise it blocks on the scheduler and records the
//! outcome back into the journal.
//!
//! ## Coroutine mode (pmap)
//!
//! When running inside a `pmap()` coroutine, `agent()` does NOT call
//! `block_on` (which would freeze the entire Lua thread). Instead it deposits
//! the task into the [`CoroutineBridge`] and calls `coroutine.yield()`. The
//! `pmap()` driver retrieves the task, dispatches it asynchronously, and
//! resumes the coroutine with a pre-built result table.

use super::journal::{record, slot_from_cache, slot_from_result};
use maestro_core::contract::event::{AgentEvent, LogLevel};
use crate::sdk::task::{build_result_table, build_task};
use crate::sdk::{PendingTask, SdkContext};
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
    let bridge = cx.coroutine_bridge.clone();

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

        // ── Check if we're inside a pmap() ──
        let in_pmap = bridge.is_in_pmap();

        if in_pmap {
            // ── Coroutine mode: deposit task + return sentinel ──
            // We can't call coroutine.yield() from within a Rust callback
            // (mlua's pcall catches the C-level yield). Instead, return a
            // sentinel table. The pmap coroutine body's Lua-side wrapper
            // detects __yield and calls coroutine.yield() from Lua.
            let pending = PendingTask {
                task,
                backend,
                cache_key,
                agent_id,
                phase_id,
            };
            let request_id = bridge.deposit(pending);

            tracing::debug!(%agent_id, request_id, "agent() returning pmap yield sentinel");

            let sentinel = lua.create_table()?;
            sentinel.set("__yield", request_id as i64)?;
            Ok(sentinel)
        } else {
            // ── Original blocking mode ──
            tracing::debug!(%agent_id, "agent() submitting to scheduler (block_on)");
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
        }
    })?;
    globals.set("agent", agent_fn)?;
    Ok(())
}
