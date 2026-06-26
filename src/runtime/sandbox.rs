//! Sandboxed mlua runtime + SDK bridge (code-design §4).
//!
//! The Runtime executes Lua orchestration scripts using mlua. It provides the
//! SDK primitives that bridge to the scheduler:
//!
//! - `agent(opts)`               — run a single agent
//! - `parallel(items, mapFn)`    — barrier fan-out, results in input order
//! - `pipeline(items, stages)`   — non-barrier streaming stages (M2)
//! - `converge(items, opts)`     — adversarial verification + voting
//! - `workflow(path, args?)`     — nested sub-workflow (M6)
//! - `phase(name, planned?)`     — progress grouping, returns phase id
//! - `phase_begin(name, planned?)` — begin a structural phase span (push)
//! - `phase_end(span_id?)`        — end the current structural phase span (pop)
//! - `log(msg, level?)`          — structured log event
//! - `budget(time_ms?, rounds?)` — runtime limits hint
//! - `report(value)`             — final workflow output
//! - `json.encode/decode`        — (de)serialization helpers
//!
//! Each primitive is registered by a `register_*_sdk` function in the
//! [`crate::runtime::sdk`] (and [`crate::runtime::converge`] /
//! [`crate::runtime::pipeline`]) modules; this file only owns the VM lifecycle
//! and the registration dispatch in [`register_sdk`].
//!
//! The SDK functions block on the async scheduler through a captured
//! [`tokio::runtime::Handle`]. Because `block_on` panics inside an async worker
//! thread, the caller MUST drive `Runtime::execute` from a blocking context
//! (e.g. `tokio::task::spawn_blocking`); see `cli::run`.
//!
//! When a [`JournalStore`] is provided, `agent`/`parallel` check for cached
//! results before submitting to the scheduler (M1 resume support) and record
//! their outputs back into the journal keyed by cache key.

use crate::core::contract::backend::RunContext;
use crate::core::journal::JournalStore;
use crate::core::Scheduler;
use crate::runtime::error::{ExecLimits, ScriptError};
use crate::runtime::sdk::convert::serde_json_to_lua;
use crate::runtime::sdk::SdkContext;
use crate::runtime::{pipeline, sdk};
use mlua::{Lua, Value};
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

/// The main runtime structure that executes Lua scripts.
///
/// All shared dependencies (scheduler, run context, journal, tokio handle) are
/// captured by the SDK closures during construction; the struct itself only
/// needs to retain the VM and the report sink.
pub struct Runtime {
    lua: Lua,
    report_sink: Arc<Mutex<Option<serde_json::Value>>>,
}

impl Runtime {
    /// Create a new Runtime with sandbox applied.
    ///
    /// `handle` is the tokio runtime handle used by SDK primitives to block on
    /// the async scheduler. It is captured here (in async context) so it can be
    /// used later from the blocking execution thread.
    pub fn new(
        scheduler: Arc<Scheduler>,
        run_ctx: RunContext,
        args: serde_json::Value,
        _limits: ExecLimits,
        journal: Option<Arc<JournalStore>>,
        handle: Handle,
    ) -> Result<Self, ScriptError> {
        tracing::info!(run_id = %run_ctx.run_id, "creating runtime");
        let lua = Lua::new();
        apply_sandbox(&lua)?;

        let report_sink = Arc::new(Mutex::new(None));

        // Set up `args` and `ctx` globals.
        let args_table = serde_json_to_lua(&lua, args)?;
        lua.globals().set("args", args_table)?;
        let ctx = lua.create_table()?;
        ctx.set("run_id", run_ctx.run_id.to_string())?;
        lua.globals().set("ctx", ctx)?;

        let cx = SdkContext::new(run_ctx, scheduler, report_sink.clone(), journal, handle);
        register_sdk(&lua, &cx)?;
        tracing::debug!(run_id = %cx.run_id(), "SDK primitives registered");

        Ok(Self { lua, report_sink })
    }

    /// Execute the script and return the report value (if any).
    ///
    /// MUST be called from a blocking context (not an async worker thread),
    /// because the SDK primitives call `Handle::block_on` internally.
    pub fn execute(&self, script: &str) -> Result<serde_json::Value, ScriptError> {
        tracing::info!("begin script execution ({} bytes)", script.len());
        let start = std::time::Instant::now();
        self.lua.load(script).exec()?;
        let elapsed = start.elapsed();
        let guard = self.report_sink.lock().unwrap();
        let has_report = guard.is_some();
        tracing::info!(elapsed_ms = elapsed.as_millis() as u64, has_report, "script execution finished");
        Ok(guard.clone().unwrap_or(serde_json::Value::Null))
    }

    /// Inject completed phase span names as the `completed_spans` Lua global,
    /// so resume scripts can skip already-finished structural units.
    pub fn set_completed_spans(&self, names: &[String]) -> Result<(), ScriptError> {
        if names.is_empty() {
            return Ok(());
        }
        let table = self.lua.create_table()?;
        for name in names {
            table.set(name.as_str(), true)?;
        }
        self.lua.globals().set("completed_spans", table)?;
        Ok(())
    }
}

/// Validates a script (syntax only) without executing it.
pub fn validate_script(script: &str) -> Result<(), ScriptError> {
    tracing::debug!(bytes = script.len(), "validating script syntax");
    let lua = Lua::new();
    lua.load(script).into_function().map(|_| ()).map_err(|e| {
        tracing::debug!(error = %e, "script validation failed");
        ScriptError::from(e)
    })
}

/// Register all SDK functions as Lua globals by dispatching to each primitive's
/// registrar. Shared dependencies travel through [`SdkContext`].
fn register_sdk(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    sdk::agent::register_agent_sdk(lua, cx)?;
    sdk::workflow::register_workflow_sdk(lua, cx)?;
    sdk::control::register_control_sdk(lua, cx)?;
    sdk::report::register_report_sdk(lua, cx)?;
    pipeline::register_pipeline_sdk(lua, cx)?;
    // converge::register_converge_sdk(lua, cx)?; // temporarily disabled
    Ok(())
}

/// Apply sandbox restrictions to the Lua VM (blocks I/O / OS / dynamic loading).
fn apply_sandbox(lua: &Lua) -> Result<(), ScriptError> {
    tracing::debug!("applying Lua sandbox restrictions");
    let globals = lua.globals();
    for name in ["io", "os", "debug", "package", "require", "loadfile", "dofile", "loadstring"] {
        let _ = globals.set(name, Value::Nil);
    }
    Ok(())
}
