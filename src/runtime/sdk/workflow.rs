//! `workflow(path, args?)` — nested sub-workflow execution (M6).
//!
//! Reads a Lua script from disk and runs it in a fresh [`Runtime`] that shares
//! this run's scheduler, context, and journal. The sub-workflow's `report()`
//! value is returned to the caller.

use crate::core::contract::event::AgentEvent;
use crate::runtime::error::ExecLimits;
use crate::runtime::sdk::convert::{lua_value_from_json, value_to_json};
use crate::runtime::sdk::SdkContext;
use crate::runtime::Runtime;
use mlua::{Lua, Table, Value};
use std::sync::atomic::Ordering;

/// Register `workflow` as a Lua global.
pub(crate) fn register_workflow_sdk(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    let globals = lua.globals();

    let sched = cx.scheduler.clone();
    let run_ctx = cx.run_ctx.clone();
    let journal = cx.journal.clone();
    let handle = cx.handle.clone();
    let events = cx.events();
    let run_id = cx.run_id();
    let span_counter = cx.span_counter.clone();
    let workflow_fn = lua.create_function(move |lua, (path, args): (String, Option<Table>)| {
        let span_id = span_counter.fetch_add(1, Ordering::Relaxed);
        tracing::info!(%path, span_id, "sub-workflow started");
        let sub_args = match args {
            Some(t) => value_to_json(Value::Table(t))?,
            None => serde_json::Value::Object(Default::default()),
        };
        let _ = events.send(AgentEvent::WorkflowStarted {
            run_id,
            span_id,
            path: path.clone(),
            args: sub_args.clone(),
        });
        let t0 = std::time::Instant::now();

        // Inner work; the guard below emits WorkflowDone on both Ok and Err paths.
        let outcome: mlua::Result<serde_json::Value> = (|| {
            tracing::debug!(%path, "reading sub-workflow script");
            let script = std::fs::read_to_string(&path)
                .map_err(|e| {
                    tracing::error!(%path, error = %e, "failed to read sub-workflow script");
                    mlua::Error::RuntimeError(format!("workflow: cannot read '{}': {}", path, e))
                })?;
            let sub = Runtime::new(
                sched.clone(), run_ctx.clone(), sub_args.clone(), ExecLimits::default(),
                journal.clone(), handle.clone(),
            )
            .map_err(|e| {
                tracing::error!(%path, error = %e, "sub-workflow init error");
                mlua::Error::RuntimeError(format!("workflow '{}' init error: {}", path, e))
            })?;
            sub.execute(&script)
                .map_err(|e| {
                    tracing::error!(%path, error = %e, "sub-workflow execution error");
                    mlua::Error::RuntimeError(format!("workflow '{}' error: {}", path, e))
                })
        })();

        let elapsed_ms = t0.elapsed().as_millis() as u64;
        tracing::info!(%path, elapsed_ms, success = outcome.is_ok(), "sub-workflow finished");
        let _ = events.send(AgentEvent::WorkflowDone {
            run_id,
            span_id,
            path: path.clone(),
            report: outcome.as_ref().ok().cloned().unwrap_or(serde_json::Value::Null),
            elapsed_ms,
            error: outcome.as_ref().err().map(|e| e.to_string()),
        });

        lua_value_from_json(lua, outcome?)
    })?;
    globals.set("workflow", workflow_fn)?;

    Ok(())
}
