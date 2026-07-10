//! `workflow(path, args?)` — nested sub-workflow execution (M6).
//!
//! Reads a Lua script from disk and runs it in a fresh [`Runtime`] that shares
//! this run's scheduler, context, and journal. The sub-workflow's `report()`
//! value is returned to the caller.

use luft_core::contract::event::AgentEvent;
use crate::error::ExecLimits;
use crate::sdk::convert::{lua_value_from_json, value_to_json};
use crate::sdk::SdkContext;
use crate::Runtime;
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
            let script = std::fs::read_to_string(&path).map_err(|e| {
                tracing::error!(%path, error = %e, "failed to read sub-workflow script");
                mlua::Error::RuntimeError(format!("workflow: cannot read '{}': {}", path, e))
            })?;
            let sub = Runtime::new(
                sched.clone(),
                run_ctx.clone(),
                sub_args.clone(),
                ExecLimits::default(),
                journal.clone(),
                handle.clone(),
            )
            .map_err(|e| {
                tracing::error!(%path, error = %e, "sub-workflow init error");
                mlua::Error::RuntimeError(format!("workflow '{}' init error: {}", path, e))
            })?;
            sub.execute(&script).map_err(|e| {
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
            report: outcome
                .as_ref()
                .ok()
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            elapsed_ms,
            error: outcome.as_ref().err().map(|e| e.to_string()),
        });

        lua_value_from_json(lua, outcome?)
    })?;
    globals.set("workflow", workflow_fn)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use luft_core::contract::RunContext;
    use luft_core::{BackendRegistry, Scheduler, SchedulerConfig};
    use std::sync::Arc;
    use std::sync::Mutex;
    use tokio::runtime::Handle;
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;

    /// Build a minimal SdkContext and return it with an event receiver.
    fn make_cx() -> (SdkContext, broadcast::Receiver<AgentEvent>) {
        let registry = BackendRegistry::new();
        let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
        let run_id = uuid::Uuid::now_v7();
        let (tx, rx) = broadcast::channel(256);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        scheduler.init_run_with(run_id, run_ctx.events.clone());
        let cx = SdkContext::new(
            run_ctx,
            scheduler,
            Arc::new(Mutex::new(None)),
            None,
            Handle::current(),
        );
        (cx, rx)
    }

    #[tokio::test]
    async fn register_creates_workflow_global() {
        let lua = Lua::new();
        let (cx, _rx) = make_cx();
        register_workflow_sdk(&lua, &cx).unwrap();
        assert!(lua.globals().get::<mlua::Function>("workflow").is_ok());
    }

    /// Escape backslashes in a path for embedding in a Lua string literal.
    /// On Windows, paths contain `\` which creates invalid Lua escape sequences.
    fn lua_str(s: &str) -> String {
        s.replace('\\', "\\\\")
    }

    #[tokio::test]
    async fn workflow_none_args_uses_empty_object() {
        let dir = tempfile::tempdir().unwrap();
        let sub_path = dir.path().join("sub.lua");
        std::fs::write(
            &sub_path,
            "meta = { reasoning = \"test\", phases = {{ label = \"work\" }} }\nfunction main() report({ args_type = type(args) }) end",
        )
        .unwrap();

        let lua = Lua::new();
        let (cx, _rx) = make_cx();
        register_workflow_sdk(&lua, &cx).unwrap();

        let path = sub_path.to_string_lossy().to_string();
        let script = format!("return workflow(\"{}\", nil)", lua_str(&path));

        let args_type: String = tokio::task::spawn_blocking(move || {
            let result: mlua::Value = lua.load(script).eval()?;
            let t = result
                .as_table()
                .cloned()
                .ok_or_else(|| mlua::Error::RuntimeError("expected table result".into()))?;
            t.get::<String>("args_type")
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(args_type, "table");
    }

    #[tokio::test]
    async fn workflow_file_not_found_returns_error() {
        let lua = Lua::new();
        let (cx, _rx) = make_cx();
        register_workflow_sdk(&lua, &cx).unwrap();

        let script = r#"return workflow("/__luft_nonexistent_test__", {})"#;

        let err = tokio::task::spawn_blocking(move || lua.load(script).eval::<mlua::Value>())
            .await
            .unwrap()
            .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("cannot read"),
            "expected 'cannot read' error, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn workflow_sub_script_runtime_error_propagates() {
        let dir = tempfile::tempdir().unwrap();
        let sub_path = dir.path().join("bad.lua");
        std::fs::write(&sub_path, "error('sub-workflow crashed')").unwrap();

        let lua = Lua::new();
        let (cx, _rx) = make_cx();
        register_workflow_sdk(&lua, &cx).unwrap();

        let path = sub_path.to_string_lossy().to_string();
        let script = format!("return workflow(\"{}\", {{}})", lua_str(&path));

        let err = tokio::task::spawn_blocking(move || lua.load(script).eval::<mlua::Value>())
            .await
            .unwrap()
            .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("sub-workflow crashed"),
            "expected execution error, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn workflow_successful_execution_returns_report() {
        let dir = tempfile::tempdir().unwrap();
        let sub_path = dir.path().join("ok.lua");
        std::fs::write(&sub_path, "meta = { reasoning = \"test\", phases = {{ label = \"work\" }} }\nfunction main() report({ value = 99 }) end").unwrap();

        let lua = Lua::new();
        let (cx, _rx) = make_cx();
        register_workflow_sdk(&lua, &cx).unwrap();

        let path = sub_path.to_string_lossy().to_string();
        let script = format!("return workflow(\"{}\", {{}})", lua_str(&path));

        let value: i64 = tokio::task::spawn_blocking(move || {
            let result: mlua::Value = lua.load(&script).eval()?;
            let t = result
                .as_table()
                .cloned()
                .ok_or_else(|| mlua::Error::RuntimeError("expected table result".into()))?;
            t.get::<i64>("value")
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(value, 99);
    }

    #[tokio::test]
    async fn workflow_passes_args_to_sub_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let sub_path = dir.path().join("args.lua");
        std::fs::write(
            &sub_path,
            "meta = { reasoning = \"test\", phases = {{ label = \"work\" }} }\nfunction main() report({ name = args.name, count = args.count }) end",
        )
        .unwrap();

        let lua = Lua::new();
        let (cx, _rx) = make_cx();
        register_workflow_sdk(&lua, &cx).unwrap();

        let path = sub_path.to_string_lossy().to_string();
        let script = format!(
            "return workflow(\"{}\", {{ name = \"hello\", count = 7 }})",
            lua_str(&path)
        );

        let (name, count): (String, i64) = tokio::task::spawn_blocking(move || {
            let result: mlua::Value = lua.load(&script).eval()?;
            let t = result
                .as_table()
                .cloned()
                .ok_or_else(|| mlua::Error::RuntimeError("expected table result".into()))?;
            Ok::<_, mlua::Error>((t.get::<String>("name")?, t.get::<i64>("count")?))
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(name, "hello");
        assert_eq!(count, 7);
    }

    #[tokio::test]
    async fn workflow_emits_started_and_done_events() {
        let dir = tempfile::tempdir().unwrap();
        let sub_path = dir.path().join("events.lua");
        std::fs::write(&sub_path, "meta = { reasoning = \"test\", phases = {{ label = \"work\" }} }\nfunction main() report({ ok = true }) end").unwrap();

        let lua = Lua::new();
        let (cx, mut rx) = make_cx();
        register_workflow_sdk(&lua, &cx).unwrap();

        let path = sub_path.to_string_lossy().to_string();
        let script = format!("workflow(\"{}\", {{}})", lua_str(&path));

        tokio::task::spawn_blocking(move || lua.load(&script).eval::<mlua::Value>())
            .await
            .unwrap()
            .unwrap();

        let mut found_started = false;
        let mut found_done = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::WorkflowStarted { .. } => found_started = true,
                AgentEvent::WorkflowDone { .. } => found_done = true,
                _ => {}
            }
        }
        assert!(found_started, "expected WorkflowStarted event");
        assert!(found_done, "expected WorkflowDone event");
    }
}
