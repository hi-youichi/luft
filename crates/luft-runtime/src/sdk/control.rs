//! Lightweight control / progress primitives: `phase`, `log`, `budget`.
//!
//! These don't touch the scheduler — they emit progress events or stash hints
//! on a Lua global. `phase()` advances the shared phase counter that
//! `agent()`/`parallel()` read for cache keys and events.

use crate::sdk::PhaseSpan;
use crate::sdk::SdkContext;
use luft_core::contract::event::{AgentEvent, LogLevel};
use mlua::{Lua, Table, Value};
use std::sync::atomic::Ordering;

/// Register `phase`, `log`, and `budget` as Lua globals.
pub(crate) fn register_control_sdk(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    let globals = lua.globals();
    let run_id = cx.run_id();

    // ---- phase(name, planned?) -> phase_id  OR  phase(opts) -> phase_id -------
    //
    // Backward-compatible two forms:
    //   phase("label", 3)                 — classic positional
    //   phase({ label="..", planned=3, description="..", role="producer" })
    {
        let events = cx.events();
        let phase_counter = cx.phase_counter.clone();
        let phase_span_stack = cx.phase_span_stack.clone();
        let phase_fn = lua.create_function(move |lua, (first, second): (Value, Option<f64>)| {
            let (label, planned, description, role) = match first {
                Value::String(s) => {
                    let label = s.to_str()?.to_string();
                    let planned = second
                        .map(|v| {
                            if v.is_nan() || v < 0.0 { 0 }
                            else if v > usize::MAX as f64 { usize::MAX }
                            else { v as usize }
                        })
                        .unwrap_or(0);
                    let description = lua
                        .globals()
                        .get::<Table>("meta")
                        .ok()
                        .and_then(|m| m.get::<Table>("phases").ok())
                        .and_then(|phases| {
                            for (_, t) in phases.pairs::<Value, Table>().flatten() {
                                let l: Option<String> = t.get("label").ok();
                                if l.as_deref() == Some(label.as_str()) {
                                    return t.get("description").ok();
                                }
                            }
                            None
                        });
                    (label, planned, description, None)
                }
                Value::Table(t) => {
                    let label: String = t.get("label")
                        .or_else(|_| t.get(1))
                        .map_err(|_| mlua::Error::RuntimeError(
                            "phase: missing 'label' field".to_string()
                        ))?;
                    let planned = t.get::<f64>("planned")
                        .or_else(|_| t.get::<f64>(2))
                        .ok()
                        .map(|v| {
                            if v.is_nan() || v < 0.0 { 0 }
                            else if v > usize::MAX as f64 { usize::MAX }
                            else { v as usize }
                        })
                        .unwrap_or(0);
                    let description: Option<String> = t.get("description").ok();
                    let role: Option<String> = t.get("role").ok();
                    (label, planned, description, role)
                }
                other => return Err(mlua::Error::RuntimeError(format!(
                    "phase: expected string or table, got {}", other.type_name()
                ))),
            };

            let phase_id = phase_counter.fetch_add(1, Ordering::Relaxed) + 1;
            let parent_span_id = {
                let stack = phase_span_stack.lock().unwrap();
                stack.last().map(|s| s.id)
            };
            tracing::info!(phase_id, %label, planned, parent_span_id = ?parent_span_id, "phase started");
            let _ = events.send(AgentEvent::PhaseStarted {
                run_id,
                phase_id,
                label,
                planned,
                parent_span_id,
                description,
                role,
                ts: chrono::Utc::now(),
            });
            Ok(phase_id as i64)
        })?;
        globals.set("phase", phase_fn)?;
    }

    // ---- phase_begin(name, planned?) -> span_id ----------------------------
    {
        let events = cx.events();
        let phase_counter = cx.phase_counter.clone();
        let phase_span_stack = cx.phase_span_stack.clone();
        let begin_fn = lua.create_function(move |_, (name, planned): (String, Option<f64>)| {
            let id = phase_counter.fetch_add(1, Ordering::Relaxed) + 1;
            let planned = planned
                .map(|v| {
                    if v.is_nan() || v < 0.0 { 0 }
                    else if v > usize::MAX as f64 { usize::MAX }
                    else { v as usize }
                })
                .unwrap_or(0);
            let (parent_id, depth) = {
                let stack = phase_span_stack.lock().unwrap();
                (stack.last().map(|s| s.id), stack.len() as u32)
            };
            let span = PhaseSpan {
                id,
                name: name.clone(),
                parent_id,
                depth,
                started_at: std::time::Instant::now(),
                planned,
            };
            tracing::info!(span_id = id, %name, parent_id = ?parent_id, depth, "phase span started");
            phase_span_stack.lock().unwrap().push(span);
            let _ = events.send(AgentEvent::PhaseSpanStarted {
                run_id,
                span_id: id,
                name,
                parent_id,
                depth,
                planned,
            });
            Ok(id as i64)
        })?;
        globals.set("phase_begin", begin_fn)?;
    }

    // ---- phase_end(span_id?) -----------------------------------------------
    {
        let events = cx.events();
        let phase_span_stack = cx.phase_span_stack.clone();
        let end_fn = lua.create_function(move |_, id: Option<i64>| {
            let span = {
                let mut stack = phase_span_stack.lock().unwrap();
                match id {
                    Some(target) => {
                        let pos = stack
                            .iter()
                            .rposition(|s| s.id as i64 == target)
                            .ok_or_else(|| {
                                mlua::Error::RuntimeError(format!(
                                    "phase_end: span id {} not found in stack",
                                    target
                                ))
                            })?;
                        stack.split_off(pos).remove(0)
                    }
                    None => stack.pop().ok_or_else(|| {
                        mlua::Error::RuntimeError("phase_end: span stack is empty".to_string())
                    })?,
                }
            };
            let elapsed_ms = span.started_at.elapsed().as_millis() as u64;
            tracing::info!(span_id = span.id, elapsed_ms, "phase span ended");
            let _ = events.send(AgentEvent::PhaseSpanDone {
                run_id,
                span_id: span.id,
                name: span.name,
                parent_id: span.parent_id,
                depth: span.depth,
                elapsed_ms,
                status: "completed".to_string(),
            });
            Ok(())
        })?;
        globals.set("phase_end", end_fn)?;
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
            tracing::trace!(?level, %msg, "script log");
            let _ = events.send(AgentEvent::Log {
                run_id,
                agent_id: None,
                level,
                msg,
            });
            Ok(())
        })?;
        globals.set("log", log_fn)?;
    }

    // ---- budget(time_ms?, max_rounds?) ------------------------------------
    {
        let events = cx.events();
        let budget_fn = lua.create_function(
            move |lua, (time_limit, max_rounds): (Option<i64>, Option<i64>)| {
                tracing::debug!(time_limit_ms = ?time_limit, ?max_rounds, "budget set");
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
                let _ = events.send(AgentEvent::BudgetSet {
                    run_id,
                    time_limit_ms: time_limit.map(|t| t.max(0) as u64),
                    max_rounds: max_rounds.map(|m| m.max(0) as u32),
                });
                Ok(())
            },
        )?;
        globals.set("budget", budget_fn)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::ReportSink;
    use luft_core::contract::backend::RunContext;
    use luft_core::contract::ids::TokenUsage;
    use luft_core::scheduler::{BackendRegistry, SchedulerConfig};
    use luft_core::Scheduler;
    use luft_core::{MockBackend, MockBehavior};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    fn test_setup() -> (Lua, SdkContext, tokio::runtime::Runtime) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let lua = Lua::new();
        let run_id = Uuid::now_v7();
        let (tx, _rx) = broadcast::channel(64);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let report_sink: ReportSink = Arc::new(Mutex::new(None));
        let handle = rt.handle().clone();
        let backend = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::from_millis(1),
            }],
        ));
        let scheduler: Arc<Scheduler> = Scheduler::new(
            SchedulerConfig::default(),
            BackendRegistry::new()
                .with(backend as Arc<dyn luft_core::contract::backend::AgentBackend>),
            None,
        );
        let cx = SdkContext::new(run_ctx, scheduler, report_sink, None, handle);
        (lua, cx, rt)
    }

    // ── phase ────────────────────────────────────────────────────

    #[test]
    fn phase_no_planned() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // No second arg → planned is None → unwrap_or(0)
        lua.load(r#"p = phase("only_label")"#).exec().unwrap();
        let id: i64 = lua.globals().get("p").unwrap();
        assert_eq!(id, 1, "first phase gets id 1");
    }

    #[test]
    fn phase_nan_planned() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        lua.load(r#"p = phase("nan", 0/0)"#).exec().unwrap();
        // NaN maps to 0 via v.is_nan() branch
        let id: i64 = lua.globals().get("p").unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn phase_negative_planned() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        lua.load(r#"p = phase("neg", -3.0)"#).exec().unwrap();
        // Negative maps to 0 via v < 0.0 branch
        let id: i64 = lua.globals().get("p").unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn phase_huge_planned() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        lua.load(r#"p = phase("huge", 1e200)"#).exec().unwrap();
        // > usize::MAX → clamped to usize::MAX
        let id: i64 = lua.globals().get("p").unwrap();
        assert_eq!(id, 1);
    }

    // ── log ───────────────────────────────────────────────────────

    #[test]
    fn log_all_levels() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // Each explicit level exercises a match arm + tracing::trace! call
        lua.load(r#"log("trace msg", "trace")"#).exec().unwrap();
        lua.load(r#"log("debug msg", "debug")"#).exec().unwrap();
        lua.load(r#"log("warn msg", "warn")"#).exec().unwrap();
        lua.load(r#"log("error msg", "error")"#).exec().unwrap();
        // Unknown level falls through to default LogLevel::Info
        lua.load(r#"log("unknown msg", "unknown")"#).exec().unwrap();
    }

    // ── budget ────────────────────────────────────────────────────

    #[test]
    fn budget_only_time() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // max_rounds = None → the if-let on line 72 is not entered
        lua.load(r#"budget(5000)"#).exec().unwrap();
        let t: mlua::Table = lua.globals().get("__budget").unwrap();
        assert_eq!(t.get::<i64>("time_limit_ms").unwrap(), 5000);
        assert!(
            t.get::<i64>("max_rounds").is_err(),
            "max_rounds must not be set"
        );
    }

    #[test]
    fn budget_only_rounds() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // time_limit = None → the if-let on line 69 is not entered
        lua.load(r#"budget(nil, 10)"#).exec().unwrap();
        let t: mlua::Table = lua.globals().get("__budget").unwrap();
        assert_eq!(t.get::<i64>("max_rounds").unwrap(), 10);
        assert!(
            t.get::<i64>("time_limit_ms").is_err(),
            "time_limit_ms must not be set"
        );
    }

    #[test]
    fn budget_no_args() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // Both None → neither if-let is entered, just creates empty table
        lua.load(r#"budget()"#).exec().unwrap();
        let t: mlua::Table = lua.globals().get("__budget").unwrap();
        assert!(t.get::<i64>("time_limit_ms").is_err());
        assert!(t.get::<i64>("max_rounds").is_err());
    }

    #[test]
    fn budget_negative_values() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // Negative → stored in table as-is, but .max(0) in the event mapping
        lua.load(r#"budget(-100, -5)"#).exec().unwrap();
        let t: mlua::Table = lua.globals().get("__budget").unwrap();
        assert_eq!(t.get::<i64>("time_limit_ms").unwrap(), -100);
        assert_eq!(t.get::<i64>("max_rounds").unwrap(), -5);
    }

    #[test]
    fn budget_table_created_on_first_call() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // __budget doesn't exist yet → unwrap_or_else creates it (line 68)
        let globals = lua.globals();
        assert!(
            globals.get::<mlua::Table>("__budget").is_err(),
            "__budget must not exist yet"
        );
        lua.load(r#"budget(100, 2)"#).exec().unwrap();
        let t: mlua::Table = globals.get("__budget").unwrap();
        assert_eq!(t.get::<i64>("time_limit_ms").unwrap(), 100);
        assert_eq!(t.get::<i64>("max_rounds").unwrap(), 2);
        // Second call reuses existing table
        lua.load(r#"budget(200, nil)"#).exec().unwrap();
        // time_limit_ms is overwritten, max_rounds keeps old value
        assert_eq!(t.get::<i64>("time_limit_ms").unwrap(), 200);
        assert_eq!(t.get::<i64>("max_rounds").unwrap(), 2);
    }

    // ── additional coverage ─────────────────────────────────────────

    #[test]
    fn phase_planned_normal() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // Valid positive f64 → hits the else branch (v as usize)
        lua.load(r#"p = phase("normal", 42.0)"#).exec().unwrap();
        let id: i64 = lua.globals().get("p").unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn phase_incrementing_ids() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // Multiple calls exercise fetch_add incrementing
        lua.load(r#"p1 = phase("first")"#).exec().unwrap();
        lua.load(r#"p2 = phase("second")"#).exec().unwrap();
        lua.load(r#"p3 = phase("third")"#).exec().unwrap();
        let p1: i64 = lua.globals().get("p1").unwrap();
        let p2: i64 = lua.globals().get("p2").unwrap();
        let p3: i64 = lua.globals().get("p3").unwrap();
        assert_eq!(p1, 1);
        assert_eq!(p2, 2);
        assert_eq!(p3, 3);
    }

    #[test]
    fn log_default_level() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // No second arg → level is None → falls to default LogLevel::Info
        lua.load(r#"log("plain message")"#).exec().unwrap();
    }

    #[test]
    fn budget_both_args() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // Both time_limit and max_rounds set in one call
        lua.load(r#"budget(3000, 20)"#).exec().unwrap();
        let t: mlua::Table = lua.globals().get("__budget").unwrap();
        assert_eq!(t.get::<i64>("time_limit_ms").unwrap(), 3000);
        assert_eq!(t.get::<i64>("max_rounds").unwrap(), 20);
    }

    // ── phase table form ────────────────────────────────────────

    #[test]
    fn phase_table_form_basic() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        lua.load(
            r#"
            p = phase({
                label = "生成分析",
                planned = 3,
                description = "多个 producer 并行分析",
                role = "producer",
            })
        "#,
        )
        .exec()
        .unwrap();
        let id: i64 = lua.globals().get("p").unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn phase_table_form_minimal() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        lua.load(r#"p = phase({ label = "only label" })"#)
            .exec()
            .unwrap();
        let id: i64 = lua.globals().get("p").unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn phase_table_missing_label_errors() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        let result = lua.load(r#"phase({ planned = 3 })"#).exec();
        assert!(result.is_err(), "missing label should error");
    }

    #[test]
    fn phase_mixed_forms_increment() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        lua.load(r#"p1 = phase("classic")"#).exec().unwrap();
        lua.load(r#"p2 = phase({ label = "table" })"#)
            .exec()
            .unwrap();
        lua.load(r#"p3 = phase("classic2", 5)"#).exec().unwrap();
        let p1: i64 = lua.globals().get("p1").unwrap();
        let p2: i64 = lua.globals().get("p2").unwrap();
        let p3: i64 = lua.globals().get("p3").unwrap();
        assert_eq!(p1, 1);
        assert_eq!(p2, 2);
        assert_eq!(p3, 3);
    }
}
