//! `workflow(path, args?)` — nested sub-workflow execution (M6).
//!
//! Reads a Lua script from disk and runs it in a fresh [`Runtime`] that shares
//! this run's scheduler, context, and journal. The sub-workflow's `report()`
//! value is returned to the caller.

use crate::runtime::error::ExecLimits;
use crate::runtime::sdk::convert::{lua_value_from_json, value_to_json};
use crate::runtime::sdk::SdkContext;
use crate::runtime::Runtime;
use mlua::{Lua, Table, Value};

/// Register `workflow` as a Lua global.
pub(crate) fn register_workflow_sdk(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    let globals = lua.globals();

    let sched = cx.scheduler.clone();
    let run_ctx = cx.run_ctx.clone();
    let journal = cx.journal.clone();
    let handle = cx.handle.clone();
    let workflow_fn = lua.create_function(move |lua, (path, args): (String, Option<Table>)| {
        let script = std::fs::read_to_string(&path)
            .map_err(|e| mlua::Error::RuntimeError(format!("workflow: cannot read '{}': {}", path, e)))?;
        let sub_args = match args {
            Some(t) => value_to_json(Value::Table(t))?,
            None => serde_json::Value::Object(Default::default()),
        };
        let sub = Runtime::new(
            sched.clone(), run_ctx.clone(), sub_args, ExecLimits::default(),
            journal.clone(), handle.clone(),
        )
        .map_err(|e| mlua::Error::RuntimeError(format!("workflow '{}' init error: {}", path, e)))?;
        let report = sub
            .execute(&script)
            .map_err(|e| mlua::Error::RuntimeError(format!("workflow '{}' error: {}", path, e)))?;
        lua_value_from_json(lua, report)
    })?;
    globals.set("workflow", workflow_fn)?;

    Ok(())
}
