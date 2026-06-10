//! Lightweight control / progress primitives: `phase`, `log`, `budget`.
//!
//! These don't touch the scheduler — they emit progress events or stash hints
//! on a Lua global. `phase()` advances the shared phase counter that
//! `agent()`/`parallel()` read for cache keys and events.

use crate::core::contract::event::{AgentEvent, LogLevel};
use crate::runtime::sdk::SdkContext;
use mlua::{Lua, Table};
use std::sync::atomic::Ordering;

/// Register `phase`, `log`, and `budget` as Lua globals.
pub(crate) fn register_control_sdk(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    let globals = lua.globals();
    let run_id = cx.run_id();

    // ---- phase(name, planned?) -> phase_id --------------------------------
    {
        let events = cx.events();
        let phase_counter = cx.phase_counter.clone();
        let phase_fn = lua.create_function(move |_, (label, planned): (String, Option<i64>)| {
            let phase_id = phase_counter.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = events.send(AgentEvent::PhaseStarted {
                run_id,
                phase_id,
                label,
                planned: planned.unwrap_or(0).max(0) as usize,
            });
            Ok(phase_id as i64)
        })?;
        globals.set("phase", phase_fn)?;
    }

    // ---- log(msg, level?) --------------------------------------------------
    {
        let events = cx.events();
        let log_fn = lua.create_function(move |_, (msg, level): (String, Option<String>)| {
            let level = match level.as_deref() {
                Some("trace") => LogLevel::Trace,
                Some("debug") => LogLevel::Debug,
                Some("warn") => LogLevel::Warn,
                Some("error") => LogLevel::Error,
                _ => LogLevel::Info,
            };
            let _ = events.send(AgentEvent::Log { run_id, agent_id: None, level, msg });
            Ok(())
        })?;
        globals.set("log", log_fn)?;
    }

    // ---- budget(time_ms?, max_rounds?) ------------------------------------
    {
        let budget_fn = lua.create_function(|lua, (time_limit, max_rounds): (Option<i64>, Option<i64>)| {
            let globals = lua.globals();
            let budget_table = globals
                .get::<Table>("__budget")
                .unwrap_or_else(|_| lua.create_table().unwrap());
            if let Some(tl) = time_limit {
                budget_table.set("time_limit_ms", tl)?;
            }
            if let Some(mr) = max_rounds {
                budget_table.set("max_rounds", mr)?;
            }
            globals.set("__budget", budget_table)?;
            Ok(())
        })?;
        globals.set("budget", budget_fn)?;
    }

    Ok(())
}
