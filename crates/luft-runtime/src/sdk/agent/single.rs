//! `agent(opts)` — run a single agent through the scheduler.
//!
//! On a journal cache hit it emits a resume log and returns the cached result
//! without re-running; otherwise it blocks on the scheduler and records the
//! outcome back into the journal.
//!
//! ## Coroutine mode (pmap)
//!
//! When running inside a `pmap()` coroutine, `agent()` does NOT call
//! `block_on` (which would freeze the entire Lua thread). Instead it deposits
//! the task into the [`CoroutineBridge`] and calls `coroutine.yield()`. The
//! `pmap()` driver retrieves the task, dispatches it asynchronously, and
//! resumes the coroutine with a pre-built result table.

use super::journal::{record, slot_from_cache, slot_from_result};
use luft_core::contract::event::{AgentEvent, LogLevel};
use crate::sdk::task::{build_result_table, build_task};
use crate::sdk::{PendingTask, SdkContext};
use mlua::{Lua, Table};
use std::sync::atomic::Ordering;

/// Register `agent` as a Lua global.
pub(super) fn register(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    let globals = lua.globals();
    let run_id = cx.run_id();
    let sched = cx.scheduler.clone();
    let journal = cx.journal.clone();
    let handle = cx.handle.clone();
    let events = cx.events();
    let phase_counter = cx.phase_counter.clone();
    let agent_seq_counter = cx.agent_seq_counter.clone();
    let bridge = cx.coroutine_bridge.clone();

    let agent_fn = lua.create_function(move |lua, opts: Table| {
        let phase_id = phase_counter.load(Ordering::Relaxed);
        let (task, cache_key, backend) = build_task(&opts, phase_id, &agent_seq_counter)?;

        // M1 resume: skip already-completed agents.
        if let Some(ref j) = journal {
            if let Some(cached) = j.get_cached(&cache_key) {
                let _ = events.send(AgentEvent::Log {
                    run_id,
                    agent_id: None,
                    level: LogLevel::Info,
                    msg: format!(
                        "resume: skip cached agent ({}…)",
                        &cache_key.hash[..8.min(cache_key.hash.len())]
                    ),
                });
                let (status, output, tokens, findings) = slot_from_cache(cached);
                return build_result_table(lua, &status, output, tokens, &findings);
            }
        }

        let agent_id = task.agent_id;

        // ── Check if we're inside a pmap() ──
        let in_pmap = bridge.is_in_pmap();

        if in_pmap {
            // ── Coroutine mode: deposit task + return sentinel ──
            // We can't call coroutine.yield() from within a Rust callback
            // (mlua's pcall catches the C-level yield). Instead, return a
            // sentinel table. The pmap coroutine body's Lua-side wrapper
            // detects __yield and calls coroutine.yield() from Lua.
            let pending = PendingTask {
                task,
                backend,
                cache_key,
                agent_id,
                phase_id,
            };
            let request_id = bridge.deposit(pending);

            tracing::debug!(%agent_id, request_id, "agent() returning pmap yield sentinel");

            let sentinel = lua.create_table()?;
            sentinel.set("__yield", request_id as i64)?;
            Ok(sentinel)
        } else {
            // ── Original blocking mode ──
            tracing::debug!(%agent_id, "agent() submitting to scheduler (block_on)");
            let result = handle
                .block_on(sched.run_agent(run_id, task, backend.as_deref()))
                .map_err(|e| {
                    tracing::error!(%agent_id, error = %e, "agent() scheduler error");
                    mlua::Error::RuntimeError(format!("agent error: {}", e))
                })?;

            tracing::debug!(%agent_id, "agent() completed");
            record(&journal, &cache_key, agent_id, phase_id, &result);

            let (status, output, tokens, findings) = slot_from_result(result);
            build_result_table(lua, &status, output, tokens, &findings)
        }
    })?;
    globals.set("agent", agent_fn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::test_support::make_sdk_context;
    use mlua::Lua;

    #[tokio::test]
    async fn register_sets_agent_global_as_callable() {
        let lua = Lua::new();
        let cx = make_sdk_context();
        register(&lua, &cx).expect("register must succeed");

        let globals = lua.globals();
        let agent: mlua::Function = globals
            .get("agent")
            .expect("agent global must be registered as a function");
        // Take a reference so we can confirm it's a function without invoking it.
        let _ = agent;
    }

    #[tokio::test]
    async fn register_does_not_set_parallel_or_pmap_globals() {
        let lua = Lua::new();
        let cx = make_sdk_context();
        register(&lua, &cx).unwrap();
        let globals = lua.globals();
        // Only the single-agent global should exist after this registrar runs.
        assert!(globals.get::<mlua::Function>("agent").is_ok());
        // parallel/pmap belong to the sibling registrars (tested in sdk/agent.rs).
        assert!(
            globals.get::<mlua::Function>("parallel").is_err(),
            "parallel must NOT be set by single::register"
        );
        assert!(
            globals.get::<mlua::Function>("pmap").is_err(),
            "pmap must NOT be set by single::register"
        );
    }

    #[tokio::test]
    async fn register_can_be_called_multiple_times_without_panic() {
        let lua = Lua::new();
        let cx = make_sdk_context();
        register(&lua, &cx).unwrap();
        register(&lua, &cx).unwrap();
        register(&lua, &cx).unwrap();
        // Final state: agent still resolves to a function (Lua globals.set
        // overwrites the previous binding rather than chaining).
        assert!(lua.globals().get::<mlua::Function>("agent").is_ok());
    }

    #[tokio::test]
    async fn agent_without_prompt_returns_runtime_error() {
        // Without a registered backend, `agent({})` would attempt to dispatch to
        // the scheduler and fail at the backend layer. The first failure mode
        // we can exercise without a backend is the missing-prompt error from
        // build_task, which propagates as a Lua RuntimeError.
        let lua = Lua::new();
        let cx = make_sdk_context();
        register(&lua, &cx).unwrap();

        let script = r#"local ok, err = pcall(agent, {}) return { ok = ok, err = tostring(err) }"#;
        let result: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(
            result.get::<bool>("ok").unwrap(),
            false,
            "agent(table) without prompt must fail"
        );
        let err = result.get::<String>("err").unwrap();
        assert!(
            err.contains("prompt"),
            "error message should mention 'prompt'; got: {err}"
        );
    }

    #[tokio::test]
    async fn register_preserves_existing_globals_by_overwriting() {
        let lua = Lua::new();
        let cx = make_sdk_context();

        // Pre-set `agent` to a sentinel noop function.
        lua.globals()
            .set(
                "agent",
                lua.create_function(|_, ()| Ok(42i64)).unwrap(),
            )
            .unwrap();

        register(&lua, &cx).unwrap();

        // After registration, `agent` resolves to the new closure (the noop
        // is overwritten, NOT chained). We can't easily exercise the new
        // closure without a backend, so we just confirm the type still
        // resolves.
        let _: mlua::Function = lua.globals().get("agent").unwrap();
    }

    // -----------------------------------------------------------------------
    // Compile-time / surface check
    // -----------------------------------------------------------------------
    #[test]
    fn single_registrar_is_in_super_module() {
        // The registrar function lives at `super::register` from inside its
        // own tests mod. Touch the symbol so a private-visibility regression
        // is caught here instead of by the first integration user.
        let _f: fn(&Lua, &SdkContext) -> mlua::Result<()> = register;
    }
}
