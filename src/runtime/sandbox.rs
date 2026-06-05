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
//! - `log(msg, level?)`          — structured log event
//! - `budget(time_ms?, rounds?)` — runtime limits hint
//! - `report(value)`             — final workflow output
//! - `json.encode/decode`        — (de)serialization helpers
//!
//! The SDK functions block on the async scheduler through a captured
//! [`tokio::runtime::Handle`]. Because `block_on` panics inside an async worker
//! thread, the caller MUST drive `Runtime::execute` from a blocking context
//! (e.g. `tokio::task::spawn_blocking`); see `cli::run`.
//!
//! When a [`JournalStore`] is provided, `agent`/`parallel` check for cached
//! results before submitting to the scheduler (M1 resume support) and record
//! their outputs back into the journal keyed by cache key.

use crate::core::contract::backend::{AgentTask, RunContext};
use crate::core::contract::event::{AgentEvent, LogLevel};
use crate::core::contract::finding::Finding;
use crate::core::contract::ids::{AgentId, PhaseId};
use crate::core::journal::{AgentCacheKey, JournalStore};
use crate::core::Scheduler;
use crate::runtime::error::{ExecLimits, ScriptError};
use crate::runtime::pipeline::{PipelineConfig, PipelineExecutor, PipelineStage};
use mlua::{Function, Lua, Table, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
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
        let lua = Lua::new();
        apply_sandbox(&lua)?;

        let report_sink = Arc::new(Mutex::new(None));

        // Set up `args` and `ctx` globals.
        let args_table = serde_json_to_lua(&lua, args)?;
        lua.globals().set("args", args_table)?;
        let ctx = lua.create_table()?;
        ctx.set("run_id", run_ctx.run_id.to_string())?;
        lua.globals().set("ctx", ctx)?;

        register_sdk(&lua, &scheduler, &run_ctx, &report_sink, &journal, &handle)?;

        Ok(Self { lua, report_sink })
    }

    /// Execute the script and return the report value (if any).
    ///
    /// MUST be called from a blocking context (not an async worker thread),
    /// because the SDK primitives call `Handle::block_on` internally.
    pub fn execute(&self, script: &str) -> Result<serde_json::Value, ScriptError> {
        self.lua.load(script).exec()?;
        let guard = self.report_sink.lock().unwrap();
        Ok(guard.clone().unwrap_or(serde_json::Value::Null))
    }
}

/// Validates a script (syntax only) without executing it.
pub fn validate_script(script: &str) -> Result<(), ScriptError> {
    let lua = Lua::new();
    lua.load(script).into_function().map(|_| ()).map_err(ScriptError::from)
}

/// Register SDK functions as Lua globals.
fn register_sdk(
    lua: &Lua,
    scheduler: &Arc<Scheduler>,
    run_ctx: &RunContext,
    report_sink: &Arc<Mutex<Option<serde_json::Value>>>,
    journal: &Option<Arc<JournalStore>>,
    handle: &Handle,
) -> mlua::Result<()> {
    let globals = lua.globals();
    let run_id = run_ctx.run_id;
    let events = run_ctx.events.clone();

    // Shared phase counter — incremented by phase(), read by agent()/parallel()
    // so cache keys and events carry a meaningful phase id.
    let phase_counter = Arc::new(AtomicU32::new(0));

    // ---- agent(opts) -------------------------------------------------------
    {
        let sched = scheduler.clone();
        let journal = journal.clone();
        let handle = handle.clone();
        let events = events.clone();
        let phase_counter = phase_counter.clone();
        let agent_fn = lua.create_function(move |lua, opts: Table| {
            let phase_id = phase_counter.load(Ordering::Relaxed);
            let (task, cache_key, backend) = build_task(&opts, phase_id)?;

            // M1 resume: skip already-completed agents.
            if let Some(ref j) = journal {
                if let Some(cached) = j.get_cached(&cache_key) {
                    let _ = events.send(AgentEvent::Log {
                        run_id,
                        agent_id: None,
                        level: LogLevel::Info,
                        msg: format!("resume: skip cached agent ({}…)", &cache_key.hash[..8.min(cache_key.hash.len())]),
                    });
                    return build_result_table(
                        lua, &cached.status, cached.output, cached.tokens, &cached.findings,
                    );
                }
            }

            let agent_id = task.agent_id;
            let result = handle
                .block_on(sched.run_agent(run_id, task, backend.as_deref()))
                .map_err(|e| mlua::Error::RuntimeError(format!("agent error: {}", e)))?;

            let status_str = format!("{:?}", result.status).to_lowercase();
            let tokens_total = result.tokens_used.total();

            if let Some(ref j) = journal {
                j.record_result(
                    &cache_key, agent_id, phase_id, result.status.clone(),
                    result.output.clone(), result.findings.clone(), result.tokens_used,
                );
            }

            build_result_table(lua, &status_str, result.output, tokens_total, &result.findings)
        })?;
        globals.set("agent", agent_fn)?;
    }

    // ---- parallel(items, mapFn) -------------------------------------------
    // Barrier fan-out: mapFn(item) -> opts table. All produced tasks run
    // concurrently under the scheduler's global semaphore; results preserve
    // input order. Cached items (resume) are filled in without re-running.
    {
        let sched = scheduler.clone();
        let journal = journal.clone();
        let handle = handle.clone();
        let phase_counter = phase_counter.clone();
        let parallel_fn = lua.create_function(move |lua, (items, map_fn): (Table, Function)| {
            let phase_id = phase_counter.load(Ordering::Relaxed);

            struct Pending {
                idx: usize,
                cache_key: AgentCacheKey,
                agent_id: AgentId,
                task: AgentTask,
                backend: Option<String>,
            }
            type Slot = (String, serde_json::Value, u64, Vec<Finding>);

            let mut slots: Vec<Option<Slot>> = Vec::new();
            let mut pending: Vec<Pending> = Vec::new();

            for item in items.sequence_values::<Value>() {
                let idx = slots.len();
                slots.push(None);
                let item = item?;
                let opts: Table = match map_fn.call(item)? {
                    Value::Table(t) => t,
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "parallel: map function must return an options table".into(),
                        ))
                    }
                };
                let (task, cache_key, backend) = build_task(&opts, phase_id)?;

                if let Some(ref j) = journal {
                    if let Some(c) = j.get_cached(&cache_key) {
                        slots[idx] = Some((c.status, c.output, c.tokens, c.findings));
                        continue;
                    }
                }
                pending.push(Pending { idx, cache_key, agent_id: task.agent_id, task, backend });
            }

            if !pending.is_empty() {
                let tasks: Vec<(AgentTask, Option<String>)> =
                    pending.iter().map(|p| (p.task.clone(), p.backend.clone())).collect();
                let results = handle.block_on(sched.run_parallel(run_id, tasks));

                for (p, res) in pending.iter().zip(results.into_iter()) {
                    let slot = match res {
                        Ok(r) => {
                            if let Some(ref j) = journal {
                                j.record_result(
                                    &p.cache_key, p.agent_id, phase_id, r.status.clone(),
                                    r.output.clone(), r.findings.clone(), r.tokens_used,
                                );
                            }
                            (format!("{:?}", r.status).to_lowercase(), r.output, r.tokens_used.total(), r.findings)
                        }
                        Err(e) => ("error".to_string(), serde_json::json!({ "error": e.to_string() }), 0, vec![]),
                    };
                    slots[p.idx] = Some(slot);
                }
            }

            let arr = lua.create_table()?;
            for (i, slot) in slots.into_iter().enumerate() {
                let (status, output, tokens, findings) =
                    slot.unwrap_or_else(|| ("error".into(), serde_json::Value::Null, 0, vec![]));
                arr.set(i + 1, build_result_table(lua, &status, output, tokens, &findings)?)?;
            }
            Ok(arr)
        })?;
        globals.set("parallel", parallel_fn)?;
    }

    // ---- converge(items, opts) --------------------------------------------
    crate::runtime::converge::register_converge_sdk(lua, scheduler, run_ctx, handle.clone())?;

    // ---- workflow(path, args?) — nested sub-workflow (M6) -----------------
    {
        let sched = scheduler.clone();
        let run_ctx = run_ctx.clone();
        let journal = journal.clone();
        let handle = handle.clone();
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
    }

    // ---- phase(name, planned?) -> phase_id --------------------------------
    {
        let events = events.clone();
        let phase_counter = phase_counter.clone();
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
        let events = events.clone();
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

    // ---- pipeline(params) — M2 streaming stages ---------------------------
    {
        let handle = handle.clone();
        let events = events.clone();
        let pipeline_fn = lua.create_function(move |lua, params: Table| {
            let events = events.clone();

            let items_raw: Vec<Value> = params.get("items").map_err(|e| {
                mlua::Error::RuntimeError(format!("pipeline: missing 'items' array: {}", e))
            })?;
            let items: Vec<serde_json::Value> = items_raw
                .into_iter()
                .map(value_to_json)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| mlua::Error::RuntimeError(format!("pipeline: item conversion error: {}", e)))?;

            if items.is_empty() {
                let t = lua.create_table()?;
                t.set("items", lua.create_table()?)?;
                t.set("ok", 0)?;
                t.set("failed", 0)?;
                return Ok(t);
            }

            let stages_table: Table = params.get("stages").map_err(|e| {
                mlua::Error::RuntimeError(format!("pipeline: missing 'stages' array: {}", e))
            })?;

            let mut stages = Vec::new();
            for i in 1..=stages_table.len()? {
                let stage_val: Value = stages_table.get(i)?;
                let (label, handler): (String, Function) = match stage_val {
                    Value::Function(func) => (format!("stage_{}", i), func),
                    Value::Table(tbl) => (tbl.get("label")?, tbl.get("handler")?),
                    _ => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "pipeline: stage {} must be a function or table",
                            i
                        )))
                    }
                };
                let label_c = label.clone();
                let stage = PipelineStage::new(&label, move |data| {
                    // `JsonArg` converts against the handler's own Lua VM (not a
                    // throwaway one), so the value is valid for `handler.call`.
                    match handler.call::<Value>(JsonArg(data)) {
                        Ok(Value::Nil) => Ok(serde_json::Value::Null),
                        Ok(v) => value_to_json(v).map_err(|e| format!("pipeline: result conversion: {}", e)),
                        Err(e) => Err(format!("pipeline: stage '{}' error: {}", label_c, e)),
                    }
                });
                stages.push(stage);
            }

            if stages.is_empty() {
                return Err(mlua::Error::RuntimeError("pipeline: at least one stage required".into()));
            }

            let max_inflight = params.get::<i64>("max_inflight").unwrap_or(5).max(1) as usize;
            let config = PipelineConfig { stages, max_inflight, ..Default::default() };

            let executor = PipelineExecutor::new(config, Some(events), run_id);
            let result = handle
                .block_on(executor.execute(items))
                .map_err(|e| mlua::Error::RuntimeError(format!("pipeline: execution error: {}", e)))?;

            let t = lua.create_table()?;
            let items_t = lua.create_table()?;
            for (i, item) in result.items.iter().enumerate() {
                let item_t = lua.create_table()?;
                item_t.set("index", item.item_index as i64)?;
                item_t.set("output", lua_value_from_json(lua, item.output.clone())?)?;
                let stages_t = lua.create_table()?;
                for (j, sr) in item.stage_results.iter().enumerate() {
                    let sr_t = lua.create_table()?;
                    sr_t.set("label", sr.label.as_str())?;
                    sr_t.set("status", format!("{:?}", sr.status))?;
                    sr_t.set("elapsed_ms", sr.elapsed_ms as i64)?;
                    stages_t.set(j + 1, sr_t)?;
                }
                item_t.set("stages", stages_t)?;
                items_t.set(i + 1, item_t)?;
            }
            t.set("items", items_t)?;
            t.set("ok", result.stats.ok as i64)?;
            t.set("failed", result.stats.failed as i64)?;
            t.set("total_stages", result.stats.total_stages as i64)?;
            t.set("total_elapsed_ms", result.stats.total_elapsed_ms as i64)?;
            Ok(t)
        })?;
        globals.set("pipeline", pipeline_fn)?;
    }

    // ---- report(value) -----------------------------------------------------
    {
        let report_sink = report_sink.clone();
        let report_fn = lua.create_function(move |_, value: Value| {
            let json = value_to_json(value)?;
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

/// Build an [`AgentTask`] (+ cache key + optional backend id) from a Lua opts
/// table. Recognised keys: `prompt` (required), `model`, `schema`, `backend`,
/// `timeout_ms`.
fn build_task(opts: &Table, phase_id: PhaseId) -> mlua::Result<(AgentTask, AgentCacheKey, Option<String>)> {
    let prompt: String = opts
        .get("prompt")
        .map_err(|_| mlua::Error::RuntimeError("agent: missing required 'prompt' field".into()))?;
    let model: Option<String> = opts.get::<Option<String>>("model").ok().flatten();
    let backend: Option<String> = opts.get::<Option<String>>("backend").ok().flatten();
    let timeout = opts
        .get::<i64>("timeout_ms")
        .ok()
        .filter(|v| *v > 0)
        .map(|v| Duration::from_millis(v as u64));
    let output_schema = match opts.get::<Value>("schema") {
        Ok(Value::Table(t)) => Some(value_to_json(Value::Table(t))?),
        Ok(Value::Boolean(b)) => Some(serde_json::Value::Bool(b)),
        _ => None,
    };

    let cache_key = AgentCacheKey::new(&prompt, model.as_deref(), phase_id);
    let task = AgentTask {
        agent_id: uuid::Uuid::now_v7(),
        phase_id,
        prompt,
        model,
        allowlist: None,
        workdir: PathBuf::from("."),
        mcp_endpoint: None,
        timeout,
        output_schema,
    };
    Ok((task, cache_key, backend))
}

/// Build the Lua result table returned to workflows by `agent()`/`parallel()`.
/// Fields: `status`, `ok`, `output`, `tokens`, `findings`.
fn build_result_table(
    lua: &Lua,
    status: &str,
    output: serde_json::Value,
    tokens: u64,
    findings: &[Finding],
) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("status", status)?;
    t.set("ok", status == "ok")?;
    t.set("output", lua_value_from_json(lua, output)?)?;
    t.set("tokens", tokens as i64)?;
    let ft = lua.create_table()?;
    for (i, f) in findings.iter().enumerate() {
        let e = lua.create_table()?;
        e.set("kind", f.kind.as_str())?;
        e.set("severity", format!("{:?}", f.severity).to_lowercase())?;
        e.set("title", f.title.as_str())?;
        e.set("detail", f.detail.as_str())?;
        ft.set(i + 1, e)?;
    }
    t.set("findings", ft)?;
    Ok(t)
}

/// A JSON value that converts into Lua lazily against the target VM.
/// Used to pass arguments to a Lua function from a `'static` closure that has
/// no direct `&Lua` handle (e.g. pipeline stage handlers).
struct JsonArg(serde_json::Value);

impl mlua::IntoLua for JsonArg {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        lua_value_from_json(lua, self.0)
    }
}

/// Convert a Lua value to a serde_json::Value.
fn value_to_json(value: Value) -> mlua::Result<serde_json::Value> {
    match value {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Boolean(b) => Ok(serde_json::Value::Bool(b)),
        Value::LightUserData(_) => Ok(serde_json::Value::Null),
        Value::Integer(i) => Ok(serde_json::Value::Number(i.into())),
        Value::Number(n) => Ok(serde_json::json!(n)),
        Value::String(s) => Ok(serde_json::Value::String(
            s.to_str().map(|s| s.to_string()).unwrap_or_default(),
        )),
        Value::Table(t) => {
            // Distinguish array-like from map-like tables.
            let len = t.raw_len();
            if len > 0 {
                let mut arr = Vec::with_capacity(len);
                for i in 1..=len {
                    arr.push(value_to_json(t.get(i)?)?);
                }
                Ok(serde_json::Value::Array(arr))
            } else {
                let mut map = serde_json::Map::new();
                for pair in t.pairs::<Value, Value>() {
                    let (k, v) = pair?;
                    let key = match k {
                        Value::String(s) => s.to_str().map(|s| s.to_string()).unwrap_or_default(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        _ => continue,
                    };
                    map.insert(key, value_to_json(v)?);
                }
                Ok(serde_json::Value::Object(map))
            }
        }
        Value::Function(_) => Ok(serde_json::Value::Null),
        Value::Thread(_) => Ok(serde_json::Value::Null),
        Value::UserData(_) => Ok(serde_json::Value::Null),
        Value::Error(e) => Err(mlua::Error::RuntimeError(format!("lua error: {}", e))),
        _ => Ok(serde_json::Value::Null),
    }
}

/// Convert a JSON string to a Lua value.
fn json_string_to_value(lua: &Lua, s: &str) -> mlua::Result<Value> {
    let json: serde_json::Value = serde_json::from_str(s)
        .map_err(|e| mlua::Error::RuntimeError(format!("json decode error: {}", e)))?;
    lua_value_from_json(lua, json)
}

/// Convert a serde_json::Value to a Lua value.
fn lua_value_from_json(lua: &Lua, json: serde_json::Value) -> mlua::Result<Value> {
    match json {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Boolean(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Integer(i))
            } else {
                Ok(Value::Number(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(lua.create_string(&s)?)),
        serde_json::Value::Array(arr) => {
            let t = lua.create_table()?;
            for (i, v) in arr.into_iter().enumerate() {
                t.set(i + 1, lua_value_from_json(lua, v)?)?;
            }
            Ok(Value::Table(t))
        }
        serde_json::Value::Object(map) => {
            let t = lua.create_table()?;
            for (k, v) in map {
                t.set(k, lua_value_from_json(lua, v)?)?;
            }
            Ok(Value::Table(t))
        }
    }
}

/// Convert a serde_json::Value to a Lua table (for the `args` global).
fn serde_json_to_lua(lua: &Lua, json: serde_json::Value) -> mlua::Result<Table> {
    match lua_value_from_json(lua, json)? {
        Value::Table(t) => Ok(t),
        _ => Ok(lua.create_table()?),
    }
}

/// Apply sandbox restrictions to the Lua VM (blocks I/O / OS / dynamic loading).
fn apply_sandbox(lua: &Lua) -> Result<(), ScriptError> {
    let globals = lua.globals();
    for name in ["io", "os", "debug", "package", "require", "loadfile", "dofile", "loadstring"] {
        let _ = globals.set(name, Value::Nil);
    }
    Ok(())
}
