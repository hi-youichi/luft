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
        let phase_fn = lua.create_function(move |_, (label, planned): (String, Option<f64>)| {
            let phase_id = phase_counter.fetch_add(1, Ordering::Relaxed) + 1;
            let planned = planned
                .map(|v| {
                    if v.is_nan() || v < 0.0 { 0 }
                    else if v > usize::MAX as f64 { usize::MAX }
                    else { v as usize }
                })
                .unwrap_or(0);
            tracing::info!(phase_id, %label, planned, "phase started");
            let _ = events.send(AgentEvent::PhaseStarted {
                run_id,
                phase_id,
                label,
                planned,
                ts: chrono::Utc::now(),
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
            tracing::trace!(?level, %msg, "script log");
            let _ = events.send(AgentEvent::Log { run_id, agent_id: None, level, msg });
            Ok(())
        })?;
        globals.set("log", log_fn)?;
    }

    // ---- budget(time_ms?, max_rounds?) ------------------------------------
    {
        let events = cx.events();
        let budget_fn = lua.create_function(move |lua, (time_limit, max_rounds): (Option<i64>, Option<i64>)| {
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
        })?;
        globals.set("budget", budget_fn)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::backend::RunContext;
    use crate::core::scheduler::{BackendRegistry, SchedulerConfig};
    use crate::core::{MockBackend, MockBehavior};
    use crate::core::contract::ids::TokenUsage;
    use crate::core::Scheduler;
    use crate::runtime::sdk::ReportSink;
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
                tokens: TokenUsage { input: 1, output: 1, cache_read: 0, cache_write: 0 },
                delay: Duration::from_millis(1),
            }],
        ));
        let scheduler: Arc<Scheduler> = Scheduler::new(
            SchedulerConfig::default(),
            BackendRegistry::new().with(backend as Arc<dyn crate::core::contract::backend::AgentBackend>),
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
        assert!(t.get::<i64>("max_rounds").is_err(), "max_rounds must not be set");
    }

    #[test]
    fn budget_only_rounds() {
        let (lua, cx, _rt) = test_setup();
        register_control_sdk(&lua, &cx).unwrap();
        // time_limit = None → the if-let on line 69 is not entered
        lua.load(r#"budget(nil, 10)"#).exec().unwrap();
        let t: mlua::Table = lua.globals().get("__budget").unwrap();
        assert_eq!(t.get::<i64>("max_rounds").unwrap(), 10);
        assert!(t.get::<i64>("time_limit_ms").is_err(), "time_limit_ms must not be set");
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
        assert!(globals.get::<mlua::Table>("__budget").is_err(), "__budget must not exist yet");
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

}
