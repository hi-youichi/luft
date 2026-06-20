//! `report(value)` and the `json` table (`json.encode` / `json.decode`).
//!
//! `report()` records the workflow's final output into the shared report sink;
//! `json` exposes (de)serialization helpers to scripts.

use crate::core::contract::event::AgentEvent;
use crate::runtime::sdk::convert::{json_string_to_value, value_to_json};
use crate::runtime::sdk::SdkContext;
use mlua::{Lua, Value};
use std::sync::atomic::Ordering;

/// Register `report` and `json` as Lua globals.
pub(crate) fn register_report_sdk(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    let globals = lua.globals();

    // ---- report(value) -----------------------------------------------------
    {
        let report_sink = cx.report_sink.clone();
        let events = cx.events();
        let run_id = cx.run_id();
        let phase_counter = cx.phase_counter.clone();
        let report_fn = lua.create_function(move |_, value: Value| {
            let json = value_to_json(value)?;
            let _ = events.send(AgentEvent::ReportEmitted {
                run_id,
                phase_id: phase_counter.load(Ordering::Relaxed),
                report: json.clone(),
            });
            *report_sink.lock().unwrap() = Some(json);
            Ok(())
        })?;
        globals.set("report", report_fn)?;
    }

    // ---- json.encode / json.decode ----------------------------------------
    {
        let json_table = lua.create_table()?;
        json_table.set(
            "encode",
            lua.create_function(|_, value: Value| {
                let json = value_to_json(value)?;
                Ok(serde_json::to_string(&json).unwrap_or_default())
            })?,
        )?;
        json_table.set(
            "decode",
            lua.create_function(|lua, s: String| json_string_to_value(lua, &s))?,
        )?;
        globals.set("json", json_table)?;
    }

    Ok(())
}
