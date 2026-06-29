//! Sandboxed mlua runtime + SDK bridge (code-design §4).
//!
//! The Runtime executes Lua orchestration scripts using mlua. It provides the
//! SDK primitives that bridge to the scheduler:
//!
//! - `agent(opts)`               — run a single agent
//! - `parallel(items, mapFn)`    — barrier fan-out, results in input order
//! - `pipeline(items, stages)`   — non-barrier streaming stages (M2)

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
//! [`crate::runtime::sdk`] (and

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
use crate::core::contract::event::{AgentEvent, PlanPhase};
use crate::core::journal::JournalStore;
use crate::core::Scheduler;
use crate::runtime::error::{ExecLimits, ScriptError};
use crate::runtime::sdk::convert::serde_json_to_lua;
use crate::runtime::sdk::SdkContext;
use crate::runtime::{pipeline, sdk};
use mlua::{Lua, Table, Value};
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
    events: tokio::sync::broadcast::Sender<AgentEvent>,
    run_id: crate::core::contract::ids::RunId,
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
        let events = cx.events();
        let run_id = cx.run_id();
        register_sdk(&lua, &cx)?;
        tracing::debug!(run_id = %run_id, "SDK primitives registered");

        Ok(Self {
            lua,
            report_sink,
            events,
            run_id,
        })
    }

    /// Execute the script and return the report value (if any).
    ///
    /// The script MUST declare a `meta = { ... }` table at the top level and a
    /// `function main() ... end` entry point. Execution proceeds in two phases:
    ///
    /// 1. **Top-level exec**: runs all assignments and function definitions
    ///    (`meta = {...}`, schema locals, `function main() ... end`). This is
    ///    safe — no agent/phase calls happen here.
    /// 2. **Extract meta**: reads the `meta` global, emits a [`AgentEvent::PlanPreview`].
    /// 3. **Call main()**: invokes the `main` function which contains the real
    ///    orchestration logic.
    ///
    /// MUST be called from a blocking context (not an async worker thread),
    /// because the SDK primitives call `Handle::block_on` internally.
    pub fn execute(&self, script: &str) -> Result<serde_json::Value, ScriptError> {
        tracing::info!("begin script execution ({} bytes)", script.len());
        let start = std::time::Instant::now();

        // Phase 1: exec top-level (meta assignment + function definitions only).
        self.lua.load(script).exec()?;

        // Phase 2: extract meta and emit PlanPreview.
        let run_id = self.run_id;
        if let Some(meta) = extract_meta(&self.lua)? {
            tracing::info!(
                phases = meta.phases.len(),
                reasoning = %meta.reasoning,
                "meta extracted"
            );
            let _ = self.events.send(AgentEvent::PlanPreview {
                run_id,
                reasoning: meta.reasoning.clone(),
                phases: meta.phases.clone(),
            });
        } else {
            tracing::warn!("no meta table found in script");
        }

        // Phase 3: call main().
        let main_fn: mlua::Function = self
            .lua
            .globals()
            .get("main")
            .map_err(|_| ScriptError::MissingMain)?;
        main_fn.call::<()>(())?;

        let elapsed = start.elapsed();
        let guard = self.report_sink.lock().unwrap();
        let has_report = guard.is_some();
        tracing::info!(
            elapsed_ms = elapsed.as_millis() as u64,
            has_report,
            "script execution finished"
        );
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
pub(crate) fn apply_sandbox(lua: &Lua) -> Result<(), ScriptError> {
    tracing::debug!("applying Lua sandbox restrictions");
    let globals = lua.globals();
    for name in [
        "io",
        "os",
        "debug",
        "package",
        "require",
        "loadfile",
        "dofile",
        "loadstring",
    ] {
        let _ = globals.set(name, Value::Nil);
    }
    Ok(())
}

/// Extracted plan metadata from the `meta` global.
pub struct WorkflowMeta {
    pub reasoning: String,
    pub phases: Vec<PlanPhase>,
}

/// Structured result of deep workflow validation.
pub struct WorkflowValidation {
    /// Extracted meta information (None if meta table is missing/invalid).
    pub meta: Option<WorkflowMeta>,
    /// Whether `main()` function is defined.
    pub has_main: bool,
    /// Whether `report(` appears in the script body (heuristic).
    pub has_report_call: bool,
    /// Whether `phase_begin()` / `phase_end()` calls are paired.
    pub span_pairing_ok: bool,
    /// All validation errors found.
    pub errors: Vec<String>,
    /// All validation warnings.
    pub warnings: Vec<String>,
}

impl WorkflowValidation {
    /// Returns `true` when there are zero errors.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Read the `meta` global table from the Lua VM after top-level exec.
///
/// Expected shape:
/// ```lua
/// meta = {
///   reasoning = "...",
///   phases = {
///     { label = "discover", dynamic = false },
///     { label = "audit",     dynamic = true  },
///   },
/// }
/// ```
pub(crate) fn extract_meta(lua: &Lua) -> Result<Option<WorkflowMeta>, ScriptError> {
    let meta_table: Table = match lua.globals().get("meta")? {
        Value::Table(t) => t,
        _ => return Ok(None),
    };

    let reasoning: String = meta_table
        .get("reasoning")
        .ok()
        .and_then(|v: Value| match v {
            Value::String(s) => s.to_str().ok().map(|s| s.to_string()),
            _ => None,
        })
        .unwrap_or_default();

    let phases_table: Table = match meta_table.get("phases")? {
        Value::Table(t) => t,
        _ => {
            tracing::warn!("meta.phases is not a table, skipping");
            return Ok(None);
        }
    };

    let mut phases = Vec::new();
    for pair in phases_table.pairs::<Value, Table>() {
        let (_, t) = pair?;
        let label: String = t
            .get("label")
            .ok()
            .and_then(|v: Value| match v {
                Value::String(s) => s.to_str().ok().map(|s| s.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| "<unlabeled>".to_string());
        let dynamic: bool = t.get("dynamic").ok().unwrap_or(false);
        phases.push(PlanPhase { label, dynamic });
    }

    Ok(Some(WorkflowMeta { reasoning, phases }))
}

/// Heuristic check that `phase_begin()` calls are accompanied by at least one
/// `phase_end()`. Dynamic loops render as a single text pair, so we only reject
/// the case where there are begins but zero ends.
pub(crate) fn check_span_pairing(script: &str) -> Result<(), String> {
    let begin_count = script.matches("phase_begin(").count();
    let end_count = script.matches("phase_end(").count();
    if begin_count > 0 && end_count == 0 {
        return Err(format!(
            "script has {} phase_begin() call(s) but no phase_end() — spans must be paired",
            begin_count
        ));
    }
    Ok(())
}

/// Register no-op stubs for SDK globals so that top-level calls in malformed
/// scripts (e.g. `agent()` outside `main()`) don't crash validation. These
/// stubs accept any arguments and return an empty table (or nothing).
fn register_validation_stubs(lua: &Lua) -> Result<(), ScriptError> {
    let globals = lua.globals();

    let empty_table = lua.create_table()?;
    let stub_ret = lua.create_function(move |_, _: mlua::MultiValue| Ok(empty_table.clone()))?;
    let stub_void = lua.create_function(|_, _: mlua::MultiValue| Ok(()))?;

    for name in [
        "agent",
        "parallel",
        "workflow",
        "phase",
        "phase_begin",
        "phase_end",
        "budget",
    ] {
        globals.set(name, stub_ret.clone())?;
    }
    globals.set("log", stub_void.clone())?;
    globals.set("report", stub_void.clone())?;

    let json = lua.create_table()?;
    json.set("encode", stub_void.clone())?;
    json.set("decode", stub_ret.clone())?;
    globals.set("json", json)?;

    Ok(())
}

/// Deep validation of a workflow script **without executing `main()`**.
///
/// Performs three layers of checks:
/// 1. **Syntax** — `Lua::load` + `exec` the top level (safe: only `meta` assignment
///    and function definitions run). SDK functions are stubbed so stray top-level
///    calls don't crash validation.
/// 2. **Structure** — verifies `meta` table exists with `reasoning` and `phases`,
///    and that `main()` is defined.
/// 3. **Heuristic** — checks for `report(` call and `phase_begin/phase_end` pairing.
///
/// Returns a [`WorkflowValidation`] regardless of semantic errors (only syntax
/// errors or internal failures produce `Err`).
pub fn validate_workflow(script: &str) -> Result<WorkflowValidation, ScriptError> {
    let lua = Lua::new();
    apply_sandbox(&lua)?;
    register_validation_stubs(&lua)?;

    // Phase 1: exec top-level (meta assignment + function definitions only).
    lua.load(script).exec()?;

    // Phase 2: extract meta.
    let meta = extract_meta(&lua)?;

    // Phase 3: check main() exists.
    let has_main = lua.globals().get::<mlua::Function>("main").is_ok();

    // Phase 4: heuristic checks.
    let has_report_call = script.contains("report(");
    let span_pairing_ok = check_span_pairing(script).is_ok();

    let mut errors = Vec::new();
    let warnings = Vec::new();

    if meta.is_none() {
        errors.push(
            "no `meta` table found (expected `meta = { reasoning = \"...\", phases = {...} }`)"
                .into(),
        );
    }
    if !has_main {
        errors.push("no `main()` function defined".into());
    }
    if !has_report_call {
        errors.push("script must call `report(...)` to emit a final result".into());
    }
    if let Err(e) = check_span_pairing(script) {
        errors.push(e);
    }

    Ok(WorkflowValidation {
        meta,
        has_main,
        has_report_call,
        span_pairing_ok,
        errors,
        warnings,
    })
}
