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
//! [`crate::sdk`] (and

//! [`crate::pipeline`]) modules; this file only owns the VM lifecycle
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

use luft_core::contract::backend::RunContext;
use luft_core::contract::event::{AgentEvent, PlanPhase};
use luft_core::journal::JournalStore;
use luft_core::Scheduler;
use crate::error::{ExecLimits, ScriptError};
use crate::sdk::convert::serde_json_to_lua;
use crate::sdk::SdkContext;
use crate::{pipeline, sdk};
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
    run_id: luft_core::contract::ids::RunId,
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
        let description: Option<String> = t.get("description").ok().and_then(|v: Value| match v {
            Value::String(s) => s.to_str().ok().map(|s| s.to_string()),
            _ => None,
        });
        phases.push(PlanPhase {
            label,
            dynamic,
            description,
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ExecLimits, ScriptError};
    use crate::sdk::test_support::make_sdk_context;
    use luft_core::contract::backend::RunContext;
    use luft_core::contract::ids::RunId;
    use luft_core::{BackendRegistry, Scheduler, SchedulerConfig};
    use mlua::Table;
    use tokio_util::sync::CancellationToken;

    /// Construct a `Runtime` with an empty backend registry. Useful for tests
    /// that exercise the VM lifecycle / sandbox / validation paths but never
    /// actually dispatch to a backend.
    fn make_runtime() -> Runtime {
        let registry = BackendRegistry::new();
        let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
        let run_id = RunId::now_v7();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        scheduler.init_run_with(run_id, run_ctx.events.clone());
        Runtime::new(
            scheduler,
            run_ctx,
            serde_json::json!({}),
            ExecLimits::default(),
            None,
            tokio::runtime::Handle::current(),
        )
        .expect("Runtime::new should succeed with an empty registry")
    }

    // -----------------------------------------------------------------------
    // WorkflowMeta / WorkflowValidation — public surface
    // -----------------------------------------------------------------------
    #[test]
    fn workflow_validation_is_valid_depends_only_on_errors() {
        let v = WorkflowValidation {
            meta: None,
            has_main: false,
            has_report_call: false,
            span_pairing_ok: false,
            errors: vec!["x".into()],
            warnings: vec!["w".into()],
        };
        assert!(!v.is_valid(), "single error must mark validation invalid");

        let v = WorkflowValidation {
            meta: None,
            has_main: false,
            has_report_call: false,
            span_pairing_ok: false,
            errors: vec![],
            warnings: vec!["w".into()],
        };
        assert!(v.is_valid(), "warnings alone must NOT mark invalid");
    }

    #[test]
    fn workflow_meta_field_access() {
        let meta = WorkflowMeta {
            reasoning: "test reasoning".into(),
            phases: vec![],
        };
        assert_eq!(meta.reasoning, "test reasoning");
        assert!(meta.phases.is_empty());
    }

    // -----------------------------------------------------------------------
    // validate_script — syntax-only validation
    // -----------------------------------------------------------------------
    #[test]
    fn validate_script_accepts_arithmetic() {
        assert!(validate_script("return 1 + 2").is_ok());
    }

    #[test]
    fn validate_script_accepts_empty_string() {
        assert!(validate_script("").is_ok());
    }

    #[test]
    fn validate_script_accepts_function_definitions() {
        assert!(validate_script("function add(a, b) return a + b end").is_ok());
    }

    #[test]
    fn validate_script_accepts_multi_line() {
        let script = r#"
            local x = 10
            local y = 20
            return x + y
        "#;
        assert!(validate_script(script).is_ok());
    }

    #[test]
    fn validate_script_rejects_unfinished_if() {
        match validate_script("if true then").unwrap_err() {
            ScriptError::Syntax(_) => {}
            other => panic!("expected Syntax, got {other:?}"),
        }
    }

    #[test]
    fn validate_script_rejects_unclosed_table() {
        match validate_script("local t = {1, 2, 3").unwrap_err() {
            ScriptError::Syntax(_) => {}
            other => panic!("expected Syntax, got {other:?}"),
        }
    }

    #[test]
    fn validate_script_rejects_garbage() {
        match validate_script("~~ not lua ~~").unwrap_err() {
            ScriptError::Syntax(_) => {}
            other => panic!("expected Syntax, got {other:?}"),
        }
    }

    #[test]
    fn validate_script_rejects_operator_error() {
        match validate_script("local x = 1 +++ 2").unwrap_err() {
            ScriptError::Syntax(_) => {}
            other => panic!("expected Syntax, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // apply_sandbox — verifies each forbidden global is removed
    // -----------------------------------------------------------------------
    #[test]
    fn apply_sandbox_removes_io_global() {
        let lua = Lua::new();
        apply_sandbox(&lua).unwrap();
        assert!(lua.globals().get::<Value>("io").is_ok());
        assert!(
            matches!(lua.globals().get::<Value>("io").unwrap(), Value::Nil),
            "io must be nil after sandbox"
        );
    }

    #[test]
    fn apply_sandbox_removes_os_global() {
        let lua = Lua::new();
        apply_sandbox(&lua).unwrap();
        match lua.globals().get::<Value>("os").unwrap() {
            Value::Nil => {}
            other => panic!("os must be nil after sandbox, got {other:?}"),
        }
    }

    #[test]
    fn apply_sandbox_removes_debug_package_require_loadfile_dofile_loadstring() {
        let lua = Lua::new();
        apply_sandbox(&lua).unwrap();
        for name in ["debug", "package", "require", "loadfile", "dofile", "loadstring"] {
            match lua.globals().get::<Value>(name).unwrap() {
                Value::Nil => {}
                other => panic!("{name} must be nil after sandbox, got {other:?}"),
            }
        }
    }

    #[test]
    fn apply_sandbox_preserves_safe_globals() {
        let lua = Lua::new();
        apply_sandbox(&lua).unwrap();
        // math / string / table / pairs / ipairs / type / print / tostring remain.
        assert!(lua.globals().get::<mlua::Function>("pairs").is_ok());
        assert!(lua.globals().get::<mlua::Function>("ipairs").is_ok());
        assert!(lua.globals().get::<mlua::Function>("type").is_ok());
        assert!(lua.globals().get::<mlua::Function>("tostring").is_ok());
        assert!(lua.globals().get::<mlua::Function>("tonumber").is_ok());
    }

    #[test]
    fn apply_sandbox_is_idempotent() {
        let lua = Lua::new();
        apply_sandbox(&lua).unwrap();
        apply_sandbox(&lua).unwrap();
        apply_sandbox(&lua).unwrap();
        // All forbidden globals still nil after multiple passes.
        for name in ["io", "os", "debug", "package", "require"] {
            match lua.globals().get::<Value>(name).unwrap() {
                Value::Nil => {}
                other => panic!("{name} must remain nil, got {other:?}"),
            }
        }
    }

    // -----------------------------------------------------------------------
    // extract_meta — every branch
    // -----------------------------------------------------------------------
    #[test]
    fn extract_meta_returns_none_when_global_missing() {
        let lua = Lua::new();
        assert!(matches!(extract_meta(&lua).unwrap(), None));
    }

    #[test]
    fn extract_meta_returns_none_when_meta_is_not_a_table() {
        let lua = Lua::new();
        lua.globals().set("meta", "oops").unwrap();
        assert!(matches!(extract_meta(&lua).unwrap(), None));
    }

    #[test]
    fn extract_meta_returns_none_when_phases_missing() {
        let lua = Lua::new();
        lua.globals()
            .set("meta", lua.create_table().unwrap())
            .unwrap();
        // meta table exists but no `phases` field.
        assert!(matches!(extract_meta(&lua).unwrap(), None));
    }

    #[test]
    fn extract_meta_with_well_formed_meta_extracts_reasoning_and_phases() {
        let lua = Lua::new();
        let meta = lua.create_table().unwrap();
        meta.set("reasoning", "do thing").unwrap();
        let phases = lua.create_table().unwrap();
        let p0 = lua.create_table().unwrap();
        p0.set("label", "discover").unwrap();
        p0.set("dynamic", false).unwrap();
        phases.set(1, p0).unwrap();
        let p1 = lua.create_table().unwrap();
        p1.set("label", "audit").unwrap();
        p1.set("dynamic", true).unwrap();
        p1.set("description", "long audit").unwrap();
        phases.set(2, p1).unwrap();
        meta.set("phases", phases).unwrap();
        lua.globals().set("meta", meta).unwrap();

        let extracted = extract_meta(&lua).unwrap().expect("meta must be Some");
        assert_eq!(extracted.reasoning, "do thing");
        assert_eq!(extracted.phases.len(), 2);
        assert_eq!(extracted.phases[0].label, "discover");
        assert!(!extracted.phases[0].dynamic);
        assert_eq!(extracted.phases[1].label, "audit");
        assert!(extracted.phases[1].dynamic);
        assert_eq!(extracted.phases[1].description.as_deref(), Some("long audit"));
    }

    #[test]
    fn extract_meta_phase_missing_label_falls_back_to_unlabeled() {
        let lua = Lua::new();
        let meta = lua.create_table().unwrap();
        let phases = lua.create_table().unwrap();
        let p0 = lua.create_table().unwrap();
        phases.set(1, p0).unwrap();
        meta.set("phases", phases).unwrap();
        lua.globals().set("meta", meta).unwrap();

        let extracted = extract_meta(&lua).unwrap().expect("meta must be Some");
        assert_eq!(extracted.phases.len(), 1);
        assert_eq!(extracted.phases[0].label, "<unlabeled>");
        assert!(!extracted.phases[0].dynamic);
    }

    #[test]
    fn extract_meta_missing_reasoning_defaults_to_empty_string() {
        let lua = Lua::new();
        let meta = lua.create_table().unwrap();
        let phases = lua.create_table().unwrap();
        meta.set("phases", phases).unwrap();
        lua.globals().set("meta", meta).unwrap();

        let extracted = extract_meta(&lua).unwrap().expect("meta must be Some");
        assert_eq!(extracted.reasoning, "");
    }

    // -----------------------------------------------------------------------
    // check_span_pairing — every branch
    // -----------------------------------------------------------------------
    #[test]
    fn check_span_pairing_empty_script_is_ok() {
        assert!(check_span_pairing("").is_ok());
    }

    #[test]
    fn check_span_pairing_no_phase_calls_is_ok() {
        let script = "function main() report({ ok = true }) end";
        assert!(check_span_pairing(script).is_ok());
    }

    #[test]
    fn check_span_pairing_only_ends_is_ok() {
        // No begins → no pairing violation.
        let script = "phase_end(1)";
        assert!(check_span_pairing(script).is_ok());
    }

    #[test]
    fn check_span_pairing_more_begins_than_ends_with_zero_ends_is_error() {
        // The function only rejects scripts where begins > 0 AND ends == 0,
        // so two begins + zero ends triggers the error, but two begins + one
        // end is already balanced enough to pass.
        let script = "phase_begin(\"a\") phase_begin(\"b\")";
        let err = check_span_pairing(script).unwrap_err();
        assert!(err.contains("phase_begin"));
        assert!(err.contains("phase_end"));
    }

    #[test]
    fn check_span_pairing_single_unpaired_begin_is_error() {
        let script = "phase_begin(\"a\")";
        let err = check_span_pairing(script).unwrap_err();
        assert!(err.contains("1 phase_begin()"));
    }

    #[test]
    fn check_span_pairing_extra_ends_do_not_count_as_paired_begins() {
        // Multiple ends with zero begins is allowed (orphan ends are not rejected).
        let script = "phase_end(1) phase_end(2)";
        assert!(check_span_pairing(script).is_ok());
    }

    // -----------------------------------------------------------------------
    // validate_workflow — every branch
    // -----------------------------------------------------------------------
    const WELL_FORMED: &str = r#"
        meta = { reasoning = "do thing", phases = { { label = "x" } } }
        function main() report({ ok = true }) end
    "#;

    #[test]
    fn validate_workflow_well_formed_is_valid() {
        let v = validate_workflow(WELL_FORMED).unwrap();
        assert!(v.is_valid(), "errors: {:?}", v.errors);
        assert!(v.has_main);
        assert!(v.has_report_call);
        assert!(v.span_pairing_ok);
        let meta = v.meta.expect("meta must be Some for well-formed script");
        assert_eq!(meta.reasoning, "do thing");
        assert_eq!(meta.phases.len(), 1);
        assert_eq!(meta.phases[0].label, "x");
    }

    #[test]
    fn validate_workflow_missing_meta_is_error() {
        let script = r#"
            function main() report({ ok = true }) end
        "#;
        let v = validate_workflow(script).unwrap();
        assert!(!v.is_valid());
        assert!(v.errors.iter().any(|e| e.contains("meta")));
    }

    #[test]
    fn validate_workflow_missing_main_is_error() {
        let script = r#"
            meta = { reasoning = "r", phases = {} }
        "#;
        let v = validate_workflow(script).unwrap();
        assert!(!v.is_valid());
        assert!(v.errors.iter().any(|e| e.contains("main")));
    }

    #[test]
    fn validate_workflow_missing_report_call_is_error() {
        let script = r#"
            meta = { reasoning = "r", phases = {} }
            function main() end
        "#;
        let v = validate_workflow(script).unwrap();
        assert!(!v.is_valid());
        assert!(v.errors.iter().any(|e| e.contains("report")));
    }

    #[test]
    fn validate_workflow_unpaired_phase_begin_is_error() {
        let script = r#"
            meta = { reasoning = "r", phases = {} }
            function main()
              phase_begin("outer")
              report({ ok = true })
            end
        "#;
        let v = validate_workflow(script).unwrap();
        assert!(!v.is_valid());
        assert!(!v.span_pairing_ok);
        assert!(v.errors.iter().any(|e| e.contains("phase_begin")));
    }

    #[test]
    fn validate_workflow_syntax_error_returns_err() {
        let result = validate_workflow("function main(");
        assert!(result.is_err());
        match result.err().expect("expected Err") {
            ScriptError::Syntax(_) => {}
            other => panic!("expected Syntax, got {other:?}"),
        }
    }

    #[test]
    fn validate_workflow_meta_can_be_nil_and_phases_omitted() {
        let script = r#"
            function main() report({}) end
        "#;
        let v = validate_workflow(script).unwrap();
        assert!(!v.is_valid());
        // Two errors: missing meta, no phases; plus main/report covered.
        assert!(v.errors.iter().any(|e| e.contains("meta")));
    }

    // -----------------------------------------------------------------------
    // Runtime lifecycle
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn runtime_new_sets_args_and_ctx_globals() {
        let runtime = make_runtime();
        let args = runtime.lua.globals().get::<Table>("args").unwrap();
        // Empty JSON object → empty Lua table.
        assert_eq!(args.raw_len(), 0);
        let ctx = runtime.lua.globals().get::<Table>("ctx").unwrap();
        let run_id_str: String = ctx.get("run_id").unwrap();
        assert!(!run_id_str.is_empty());
    }

    #[tokio::test]
    async fn runtime_new_sets_globals_like_math_string_table() {
        let runtime = make_runtime();
        // Untouched Lua globals should be preserved (sandbox doesn't strip them).
        let g = runtime.lua.globals();
        assert!(g.get::<mlua::Function>("pairs").is_ok());
        assert!(g.get::<Table>("math").is_ok());
        assert!(g.get::<Table>("string").is_ok());
        assert!(g.get::<Table>("table").is_ok());
    }

    #[tokio::test]
    async fn runtime_new_does_not_expose_sandbox_globals() {
        let runtime = make_runtime();
        for forbidden in ["io", "os", "debug", "package", "require", "loadfile"] {
            match runtime.lua.globals().get::<Value>(forbidden).unwrap() {
                Value::Nil => {}
                other => panic!("{forbidden} must be nil after Runtime::new, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn runtime_set_completed_spans_empty_input_is_noop() {
        let runtime = make_runtime();
        // No-op for empty input: must not create the global.
        runtime.set_completed_spans(&[]).unwrap();
        // The global is absent (set only when non-empty), but accessing it as
        // mlua::Value should not panic — we just confirm a non-error path.
        let _ = runtime.lua.globals().get::<Value>("completed_spans");
    }

    #[tokio::test]
    async fn runtime_set_completed_spans_creates_global_with_truthy_names() {
        let runtime = make_runtime();
        runtime
            .set_completed_spans(&["explore".into(), "audit".into()])
            .unwrap();
        let t: Table = runtime
            .lua
            .globals()
            .get("completed_spans")
            .expect("completed_spans global must exist after set");
        // Use string-key lookup instead of raw_len (which counts sequence
        // entries, but the runtime sets string keys).
        assert!(matches!(t.get::<bool>("explore").unwrap(), true));
        assert!(matches!(t.get::<bool>("audit").unwrap(), true));
        // Iterate to confirm both keys are present.
        let mut seen = 0;
        for _ in t.clone().pairs::<String, bool>() {
            seen += 1;
        }
        assert_eq!(seen, 2);
    }

    #[tokio::test]
    async fn runtime_execute_well_formed_script_returns_report_value() {
        let runtime = make_runtime();
        let script = r#"
            meta = { reasoning = "ok", phases = { { label = "do" } } }
            function main() report({ value = 42, kind = "unit" }) end
        "#;
        let report = runtime
            .execute(script)
            .expect("well-formed script must execute");
        assert_eq!(report["value"], 42);
        assert_eq!(report["kind"], "unit");
    }

    #[tokio::test]
    async fn runtime_execute_returns_null_when_report_never_called() {
        let runtime = make_runtime();
        let script = r#"
            meta = { reasoning = "ok", phases = {} }
            function main() end
        "#;
        // validate_workflow would reject this, but Runtime::execute itself
        // still runs — it should return Null when no report() is called.
        let report = runtime
            .execute(script)
            .expect("script missing report() still executes; result is null");
        assert_eq!(report, serde_json::Value::Null);
    }

    #[tokio::test]
    async fn runtime_execute_missing_main_returns_missing_main_error() {
        let runtime = make_runtime();
        // Top-level-only assigns meta but never defines main().
        let script = r#"
            meta = { reasoning = "ok", phases = {} }
        "#;
        match runtime.execute(script).unwrap_err() {
            ScriptError::MissingMain => {}
            other => panic!("expected MissingMain, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn runtime_execute_syntax_error_surfaces_syntax_variant() {
        let runtime = make_runtime();
        let script = "function main( report({}) end"; // unclosed paren
        match runtime.execute(script).unwrap_err() {
            ScriptError::Syntax(_) => {}
            other => panic!("expected Syntax, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn runtime_execute_meta_table_emits_plan_preview_event() {
        let runtime = make_runtime();
        // Subscribe *after* construction so we only see events emitted during execute().
        let mut rx = runtime.events.subscribe();
        let script = r#"
            meta = { reasoning = "p", phases = { { label = "L" } } }
            function main() report({ ok = true }) end
        "#;
        runtime.execute(script).unwrap();
        let ev = rx.try_recv().expect("PlanPreview must be emitted");
        match ev {
            AgentEvent::PlanPreview {
                reasoning,
                phases,
                ..
            } => {
                assert_eq!(reasoning, "p");
                assert_eq!(phases.len(), 1);
                assert_eq!(phases[0].label, "L");
            }
            other => panic!("expected PlanPreview, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Public-API surface compile-time checks
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn public_api_surface() {
        // Touch each public type/function once so a future rename / visibility
        // change is caught at compile time.
        let _: fn(&str) -> Result<WorkflowValidation, ScriptError> = validate_workflow;
        let _: fn(&str) -> Result<(), ScriptError> = validate_script;
        // WorkflowMeta / WorkflowValidation are plain structs — make sure the
        // field shapes still match the call sites in the rest of the runtime.
        let _m = WorkflowMeta {
            reasoning: "".into(),
            phases: vec![],
        };
        let _v = WorkflowValidation {
            meta: None,
            has_main: false,
            has_report_call: false,
            span_pairing_ok: true,
            errors: vec![],
            warnings: vec![],
        };
        // Public methods on Runtime stay callable.
        let rt = make_runtime();
        let _: Result<(), ScriptError> = rt.set_completed_spans(&["a".into()]);
    }

    // -----------------------------------------------------------------------
    // Helper-touching the SdkContext helper to ensure test_support compiles
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_support_helper_builds_sdk_context() {
        let _cx = make_sdk_context();
    }
}
