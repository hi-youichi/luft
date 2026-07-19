//! `report(value)` and the `json` table (`json.encode` / `json.decode`).
//!
//! `report()` records the workflow's final output into the shared report sink;
//! `json` exposes (de)serialization helpers to scripts.

use crate::sdk::convert::{json_string_to_value, value_to_json};
use crate::sdk::SdkContext;
use luft_core::contract::event::AgentEvent;
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
        let (lua, cx, rt, _) = test_setup_with_rx();
        (lua, cx, rt)
    }

    fn test_setup_with_rx() -> (
        Lua,
        SdkContext,
        tokio::runtime::Runtime,
        broadcast::Receiver<AgentEvent>,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let lua = Lua::new();
        let run_id = Uuid::now_v7();
        let (tx, rx) = broadcast::channel(64);
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
        (lua, cx, rt, rx)
    }

    #[test]
    fn report_stores_table_in_sink() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        lua.load(r#"report({ key = "value", num = 42 })"#)
            .exec()
            .unwrap();

        let sink = cx.report_sink.lock().unwrap();
        let stored = sink.as_ref().expect("report sink should be Some");
        assert_eq!(stored["key"], "value");
        assert_eq!(stored["num"], 42);
    }

    #[test]
    fn report_stores_scalar_in_sink() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        lua.load(r#"report("hello")"#).exec().unwrap();

        let sink = cx.report_sink.lock().unwrap();
        let stored = sink.as_ref().expect("report sink should be Some");
        assert_eq!(stored.as_str().unwrap(), "hello");
    }

    #[test]
    fn report_overwrites_previous_value() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        lua.load(r#"report({ first = true })"#).exec().unwrap();
        lua.load(r#"report({ second = true })"#).exec().unwrap();

        let sink = cx.report_sink.lock().unwrap();
        let stored = sink.as_ref().expect("report sink should be Some");
        assert_eq!(stored["second"], true);
        assert!(
            stored.get("first").is_none(),
            "previous value should be replaced"
        );
    }

    #[test]
    fn json_encode_serializes_table() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        let result: String = lua
            .load(r#"return json.encode({ a = 1, b = 2 })"#)
            .eval()
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
        assert_eq!(parsed["b"], 2);
    }

    #[test]
    fn json_encode_nil_returns_null() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        let result: String = lua.load(r#"return json.encode(nil)"#).eval().unwrap();
        assert_eq!(result, "null");
    }

    #[test]
    fn json_decode_parses_object() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        let result: mlua::Value = lua
            .load(r#"return json.decode('{"x": 10, "y": "hello"}')"#)
            .eval()
            .unwrap();
        let json = value_to_json(result).unwrap();
        assert_eq!(json["x"], 10);
        assert_eq!(json["y"], "hello");
    }

    #[test]
    fn json_decode_invalid_string_errors() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        let result: mlua::Result<mlua::Value> =
            lua.load(r#"return json.decode("{invalid}")"#).eval();
        assert!(result.is_err(), "decode should fail on invalid json");
    }

    // -----------------------------------------------------------------------
    // report(nil)
    // -----------------------------------------------------------------------

    #[test]
    fn report_nil_stores_null() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        lua.load("report(nil)").exec().unwrap();

        let sink = cx.report_sink.lock().unwrap();
        assert_eq!(*sink, Some(serde_json::Value::Null));
    }

    // -----------------------------------------------------------------------
    // value_to_json error propagation  (report / json.encode)
    // -----------------------------------------------------------------------

    #[test]
    fn report_and_json_encode_propagate_value_to_json_errors() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        // value_to_json returns Err only for Value::Error (e.g. a caught but
        // non-serialisable Lua error object).  Both report() and json.encode()
        // use value_to_json(value)?, so the error propagates to Lua.
        let err_val = Value::Error(Box::new(mlua::Error::RuntimeError("bad".into())));
        assert!(value_to_json(err_val).is_err());

        // Verify the same path is exercised inside report() by calling it through
        // Lua with a value that mlua preserves as Value::Error.
        let globals = lua.globals();
        if globals
            .set(
                "__err",
                Value::Error(Box::new(mlua::Error::RuntimeError("bad".into()))),
            )
            .is_ok()
        {
            assert!(lua.load("report(__err)").exec().is_err());
            assert!(lua
                .load("return json.encode(__err)")
                .eval::<String>()
                .is_err());
        }
    }

    // -----------------------------------------------------------------------
    // json.decode  —  edge cases (arrays, strings, numbers, booleans, null)
    // -----------------------------------------------------------------------

    #[test]
    fn json_decode_parses_array() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        let result: mlua::Value = lua
            .load(r#"return json.decode('[1, 2, 3]')"#)
            .eval()
            .unwrap();
        let json = value_to_json(result).unwrap();
        assert_eq!(json, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn json_decode_parses_string() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        let result: String = lua.load(r#"return json.decode('"hello"')"#).eval().unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn json_decode_parses_number() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        let result: i64 = lua.load(r#"return json.decode('42')"#).eval().unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn json_decode_parses_boolean() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        let result: bool = lua.load(r#"return json.decode('true')"#).eval().unwrap();
        assert!(result);
    }

    #[test]
    fn json_decode_parses_null() {
        let (lua, cx, _rt) = test_setup();
        register_report_sdk(&lua, &cx).unwrap();

        let result: mlua::Value = lua.load(r#"return json.decode('null')"#).eval().unwrap();
        assert!(matches!(result, mlua::Value::Nil));
    }

    // -----------------------------------------------------------------------
    // AgentEvent::ReportEmitted
    // -----------------------------------------------------------------------

    #[test]
    fn report_emits_agent_event() {
        let (lua, cx, _rt, mut rx) = test_setup_with_rx();
        register_report_sdk(&lua, &cx).unwrap();

        lua.load(r#"report({ key = "val" })"#).exec().unwrap();

        let event = rx.try_recv().expect("should receive ReportEmitted");
        match event {
            AgentEvent::ReportEmitted {
                run_id,
                phase_id: _,
                report,
            } => {
                assert_eq!(run_id, cx.run_id());
                assert_eq!(report["key"], "val");
            }
            other => panic!("expected ReportEmitted, got {other:?}"),
        }
    }
}
