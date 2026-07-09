//! `parallel(items, mapFn)` — barrier fan-out over the scheduler.
//!
//! `mapFn(item) -> opts` produces a task per item; all non-cached tasks run
//! concurrently under the scheduler's global semaphore and results are returned
//! in input order. Cached items (resume) are filled in without re-running.

use super::journal::{record, slot_from_cache, slot_from_result, Slot};
use maestro_core::contract::backend::AgentTask;
use maestro_core::contract::event::AgentEvent;
use maestro_core::contract::ids::AgentId;
use maestro_core::journal::AgentCacheKey;
use crate::sdk::task::{build_result_table, build_task};
use crate::sdk::SdkContext;
use mlua::{Function, Lua, Table, Value};
use std::sync::atomic::Ordering;

/// Register `parallel` as a Lua global.
pub(super) fn register(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    let globals = lua.globals();
    let run_id = cx.run_id();
    let sched = cx.scheduler.clone();
    let journal = cx.journal.clone();
    let handle = cx.handle.clone();
    let phase_counter = cx.phase_counter.clone();
    let agent_seq_counter = cx.agent_seq_counter.clone();
    let span_counter = cx.span_counter.clone();
    let events = cx.events();

    let parallel_fn = lua.create_function(move |lua, (items, map_fn): (Table, Function)| {
        let phase_id = phase_counter.load(Ordering::Relaxed);
        let span_id = span_counter.fetch_add(1, Ordering::Relaxed);
        let count = items.raw_len();
        tracing::debug!(count, phase_id, "parallel() fan-out started");
        let _ = events.send(AgentEvent::ParallelStarted { run_id, phase_id, span_id, count });
        let t0 = std::time::Instant::now();

        struct Pending {
            idx: usize,
            cache_key: AgentCacheKey,
            agent_id: AgentId,
            task: AgentTask,
            backend: Option<String>,
        }

        // Inner work; the guard below emits ParallelDone on both Ok and Err paths.
        let outcome: mlua::Result<(Table, usize, usize, serde_json::Value)> = (|| {
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
                let (task, cache_key, backend) = build_task(&opts, phase_id, &agent_seq_counter)?;

                if let Some(ref j) = journal {
                    if let Some(c) = j.get_cached(&cache_key) {
                        slots[idx] = Some(slot_from_cache(c));
                        continue;
                    }
                }
                pending.push(Pending { idx, cache_key, agent_id: task.agent_id, task, backend });
            }

            if !pending.is_empty() {
                let n_pending = pending.len();
                let n_cached = count - n_pending;
                tracing::debug!(n_pending, n_cached, "parallel() dispatching to scheduler");
                let tasks: Vec<(AgentTask, Option<String>)> =
                    pending.iter().map(|p| (p.task.clone(), p.backend.clone())).collect();
                let results = handle.block_on(sched.run_parallel(run_id, tasks));

                for (p, res) in pending.iter().zip(results) {
                    let slot = match res {
                        Ok(r) => {
                            tracing::debug!(agent_id = %p.agent_id, "parallel() agent completed");
                            record(&journal, &p.cache_key, p.agent_id, phase_id, &r);
                            slot_from_result(r)
                        }
                        Err(e) => {
                            tracing::warn!(agent_id = %p.agent_id, error = %e, "parallel() agent failed");
                            ("error".to_string(), serde_json::json!({ "error": e.to_string() }), 0, vec![])
                        }
                    };
                    slots[p.idx] = Some(slot);
                }
            }

            // Aggregate full per-item results + counts for ParallelDone (E).
            let default: Slot = ("error".into(), serde_json::Value::Null, 0, vec![]);
            let results_json: Vec<serde_json::Value> = slots
                .iter()
                .map(|s| {
                    let (status, output, tokens, findings) = s.as_ref().unwrap_or(&default);
                    serde_json::json!({
                        "status": status,
                        "output": output,
                        "tokens": tokens,
                        "findings": findings,
                    })
                })
                .collect();
            let ok = slots.iter().filter(|s| s.as_ref().map(|x| x.0 == "ok").unwrap_or(false)).count();
            let failed = slots.len() - ok;

            let arr = lua.create_table()?;
            for (i, slot) in slots.into_iter().enumerate() {
                let (status, output, tokens, findings) =
                    slot.unwrap_or_else(|| ("error".into(), serde_json::Value::Null, 0, vec![]));
                arr.set(i + 1, build_result_table(lua, &status, output, tokens, &findings)?)?;
            }
            Ok((arr, ok, failed, serde_json::Value::Array(results_json)))
        })();

        let elapsed_ms = t0.elapsed().as_millis() as u64;
        let done = match &outcome {
            Ok((_, ok, failed, results)) => AgentEvent::ParallelDone {
                run_id, phase_id, span_id, ok: *ok, failed: *failed, results: results.clone(), elapsed_ms,
            },
            Err(e) => AgentEvent::ParallelDone {
                run_id, phase_id, span_id, ok: 0, failed: count,
                results: serde_json::json!({ "error": e.to_string() }), elapsed_ms,
            },
        };
        let _ = events.send(done);
        outcome.map(|(arr, ..)| arr)
    })?;
    globals.set("parallel", parallel_fn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::register;
    use maestro_core::contract::backend::{AgentStatus, RunContext};
    use maestro_core::contract::ids::TokenUsage;
    use maestro_core::journal::JournalStore;
    use maestro_core::scheduler::{BackendRegistry, SchedulerConfig};
    use maestro_core::Scheduler;
    use maestro_core::{FailKind, MockBackend, MockBehavior};
    use crate::sdk::task::build_task;
    use crate::sdk::ReportSink;
    use crate::sdk::SdkContext;
    use mlua::Lua;
    use std::sync::atomic::AtomicU32;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    fn make_cx(behaviors: Vec<MockBehavior>) -> (Lua, SdkContext, tokio::runtime::Runtime) {
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
        let backend = Arc::new(MockBackend::new("mock", behaviors));
        let scheduler: Arc<Scheduler> = Scheduler::new(
            SchedulerConfig::default(),
            BackendRegistry::new()
                .with(backend as Arc<dyn maestro_core::contract::backend::AgentBackend>),
            None,
        );
        scheduler.init_run_with(run_id, run_ctx.events.clone());
        let cx = SdkContext::new(run_ctx, scheduler, report_sink, None, handle);
        (lua, cx, rt)
    }

    fn make_cx_with_journal(
        behaviors: Vec<MockBehavior>,
    ) -> (
        Lua,
        SdkContext,
        tokio::runtime::Runtime,
        Arc<JournalStore>,
        tempfile::TempDir,
    ) {
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
        let backend = Arc::new(MockBackend::new("mock", behaviors));
        let scheduler: Arc<Scheduler> = Scheduler::new(
            SchedulerConfig::default(),
            BackendRegistry::new()
                .with(backend as Arc<dyn maestro_core::contract::backend::AgentBackend>),
            None,
        );
        scheduler.init_run_with(run_id, run_ctx.events.clone());

        let dir = tempfile::TempDir::new().unwrap();
        let journal = Arc::new(JournalStore::new(dir.path()).unwrap());
        journal.init_run(run_id, "test").unwrap();

        let cx = SdkContext::new(
            run_ctx,
            scheduler,
            report_sink,
            Some(journal.clone()),
            handle,
        );
        (lua, cx, rt, journal, dir)
    }

    /// Like `make_cx_with_journal` but also returns the `Arc<MockBackend>` so
    /// callers can query call_count and other internals.
    fn make_cx_with_journal_and_backend(
        behaviors: Vec<MockBehavior>,
    ) -> (
        Lua,
        SdkContext,
        tokio::runtime::Runtime,
        Arc<JournalStore>,
        tempfile::TempDir,
        Arc<MockBackend>,
    ) {
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
        let backend = Arc::new(MockBackend::new("mock", behaviors));
        let scheduler: Arc<Scheduler> = Scheduler::new(
            SchedulerConfig::default(),
            BackendRegistry::new()
                .with(backend.clone() as Arc<dyn maestro_core::contract::backend::AgentBackend>),
            None,
        );
        scheduler.init_run_with(run_id, run_ctx.events.clone());

        let dir = tempfile::TempDir::new().unwrap();
        let journal = Arc::new(JournalStore::new(dir.path()).unwrap());
        journal.init_run(run_id, "test").unwrap();

        let cx = SdkContext::new(
            run_ctx,
            scheduler,
            report_sink,
            Some(journal.clone()),
            handle,
        );
        (lua, cx, rt, journal, dir, backend)
    }

    // ── empty items table ──────────────────────────────────────────

    #[test]
    fn empty_items() {
        let (lua, cx, _rt) = make_cx(vec![MockBehavior::Success {
            output: serde_json::json!({}),
            tokens: TokenUsage::default(),
            delay: Duration::ZERO,
        }]);
        register(&lua, &cx).unwrap();
        let results: mlua::Table = lua
            .load(r#"parallel({}, function() return { prompt = "x" } end)"#)
            .eval()
            .unwrap();
        assert_eq!(results.raw_len(), 0);
    }

    // ── map-fn raises a Lua error ──────────────────────────────────

    #[test]
    fn map_fn_lua_error() {
        let (lua, cx, _rt) = make_cx(vec![MockBehavior::Success {
            output: serde_json::json!({}),
            tokens: TokenUsage::default(),
            delay: Duration::ZERO,
        }]);
        register(&lua, &cx).unwrap();
        let err = lua
            .load(r#"parallel({1, 2}, function() error('boom') end)"#)
            .eval::<mlua::Value>()
            .unwrap_err();
        assert!(
            err.to_string().contains("boom"),
            "unexpected error: {}",
            err
        );
    }

    // ── build_task rejects missing prompt ──────────────────────────

    #[test]
    fn build_task_missing_prompt() {
        let (lua, cx, _rt) = make_cx(vec![MockBehavior::Success {
            output: serde_json::json!({}),
            tokens: TokenUsage::default(),
            delay: Duration::ZERO,
        }]);
        register(&lua, &cx).unwrap();
        let err = lua
            .load(r#"parallel({1}, function() return { model = "x" } end)"#)
            .eval::<mlua::Value>()
            .unwrap_err();
        assert!(
            err.to_string().contains("missing required 'prompt'"),
            "unexpected error: {}",
            err,
        );
    }

    // ── all items fail ─────────────────────────────────────────────

    #[test]
    fn all_items_fail() {
        let (lua, cx, _rt) = make_cx(vec![MockBehavior::Fail {
            kind: FailKind::Protocol,
            delay: Duration::ZERO,
        }]);
        register(&lua, &cx).unwrap();
        let script = r#"
            return parallel(
                { "a", "b", "c" },
                function(item)
                    return { prompt = "task_" .. item }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 3);
        let mut ok = 0usize;
        let mut err = 0usize;
        for i in 1..=3 {
            let r: mlua::Table = results.get(i).unwrap();
            if r.get::<bool>("ok").unwrap() {
                ok += 1;
            } else {
                err += 1;
            }
        }
        assert_eq!(ok, 0, "expected 0 ok results");
        assert_eq!(err, 3, "expected 3 error results");
    }

    // ── error detail content in partial failure ────────────────────

    #[test]
    fn partial_failure_detail() {
        let (lua, cx, _rt) = make_cx(vec![
            MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            },
            MockBehavior::Fail {
                kind: FailKind::Protocol,
                delay: Duration::ZERO,
            },
            MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            },
        ]);
        register(&lua, &cx).unwrap();
        let script = r#"
            return parallel(
                { "x", "y", "z" },
                function(item)
                    return { prompt = "task_" .. item }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 3);
        for i in 1..=3 {
            let r: mlua::Table = results.get(i).unwrap();
            if !r.get::<bool>("ok").unwrap() {
                assert_eq!(r.get::<String>("status").unwrap(), "error");
                let output: mlua::Table = r.get("output").unwrap();
                let err_msg: String = output.get("error").unwrap();
                assert!(
                    err_msg.contains("mock protocol"),
                    "expected 'mock protocol' in error detail, got: {}",
                    err_msg,
                );
            }
        }
    }

    // ── verify output values and token counts in results ───────────

    #[test]
    fn successful_output_and_tokens() {
        let (lua, cx, _rt) = make_cx(vec![
            MockBehavior::Success {
                output: serde_json::json!({ "result": "alpha" }),
                tokens: TokenUsage {
                    input: 10,
                    output: 20,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::ZERO,
            },
            MockBehavior::Success {
                output: serde_json::json!({ "result": "beta" }),
                tokens: TokenUsage {
                    input: 5,
                    output: 15,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::ZERO,
            },
        ]);
        register(&lua, &cx).unwrap();
        let script = r#"
            return parallel(
                { "a", "b" },
                function(item)
                    return { prompt = "task_" .. item }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 2);

        let r1: mlua::Table = results.get(1).unwrap();
        assert_eq!(r1.get::<String>("status").unwrap(), "ok");
        assert!(r1.get::<bool>("ok").unwrap());
        let out1: mlua::Table = r1.get("output").unwrap();
        assert_eq!(out1.get::<String>("result").unwrap(), "alpha");
        assert_eq!(r1.get::<i64>("tokens").unwrap(), 30);

        let r2: mlua::Table = results.get(2).unwrap();
        assert_eq!(r2.get::<String>("status").unwrap(), "ok");
        assert!(r2.get::<bool>("ok").unwrap());
        let out2: mlua::Table = r2.get("output").unwrap();
        assert_eq!(out2.get::<String>("result").unwrap(), "beta");
        assert_eq!(r2.get::<i64>("tokens").unwrap(), 20);
    }

    // ── single item ────────────────────────────────────────────────

    #[test]
    fn single_item() {
        let (lua, cx, _rt) = make_cx(vec![MockBehavior::Success {
            output: serde_json::json!({ "x": 42 }),
            tokens: TokenUsage {
                input: 1,
                output: 1,
                cache_read: 0,
                cache_write: 0,
            },
            delay: Duration::ZERO,
        }]);
        register(&lua, &cx).unwrap();
        let script = r#"
            return parallel(
                { "only" },
                function(item)
                    return { prompt = "single" }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 1);
        let r: mlua::Table = results.get(1).unwrap();
        assert_eq!(r.get::<String>("status").unwrap(), "ok");
        assert!(r.get::<bool>("ok").unwrap());
        assert_eq!(r.get::<i64>("tokens").unwrap(), 2);
        let out: mlua::Table = r.get("output").unwrap();
        assert_eq!(out.get::<i64>("x").unwrap(), 42);
    }

    // ── map-fn must return a table ─────────────────────────────────

    #[test]
    fn map_fn_returns_non_table_error() {
        let (lua, cx, _rt) = make_cx(vec![MockBehavior::Success {
            output: serde_json::json!({}),
            tokens: TokenUsage::default(),
            delay: Duration::ZERO,
        }]);
        register(&lua, &cx).unwrap();
        let err = lua
            .load(r#"parallel({1}, function() return 42 end)"#)
            .eval::<mlua::Value>()
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("map function must return an options table"),
            "unexpected error: {}",
            err,
        );
    }

    // ── happy path ─────────────────────────────────────────────────

    #[test]
    fn successful_parallel() {
        let (lua, cx, _rt) = make_cx(vec![
            MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::from_millis(1),
            },
            MockBehavior::Success {
                output: serde_json::json!({ "x": 1 }),
                tokens: TokenUsage {
                    input: 2,
                    output: 3,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::from_millis(1),
            },
        ]);
        register(&lua, &cx).unwrap();
        let script = r#"
            return parallel(
                { "a", "b" },
                function(item)
                    if item == "a" then
                        return { prompt = "hello", model = "claude-x" }
                    else
                        return { prompt = "world", model = "claude-x" }
                    end
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 2);
        for i in 1..=2 {
            let r: mlua::Table = results.get(i).unwrap();
            assert_eq!(r.get::<String>("status").unwrap(), "ok");
            assert!(r.get::<bool>("ok").unwrap());
        }
    }

    // ── scheduler returns errors for some agents ───────────────────

    #[test]
    fn partial_failure() {
        let (lua, cx, _rt) = make_cx(vec![
            MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::ZERO,
            },
            MockBehavior::Fail {
                kind: FailKind::Protocol,
                delay: Duration::ZERO,
            },
            MockBehavior::Success {
                output: serde_json::json!({ "z": 9 }),
                tokens: TokenUsage {
                    input: 3,
                    output: 4,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::ZERO,
            },
        ]);
        register(&lua, &cx).unwrap();
        let script = r#"
            return parallel(
                { "x", "y", "z" },
                function(item)
                    return { prompt = "task_" .. item }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 3);
        let mut ok = 0usize;
        let mut err = 0usize;
        for i in 1..=3 {
            let r: mlua::Table = results.get(i).unwrap();
            if r.get::<bool>("ok").unwrap() {
                ok += 1;
            } else {
                err += 1;
            }
        }
        assert_eq!(ok, 2, "expected 2 ok results");
        assert_eq!(err, 1, "expected 1 error result");
    }

    // ── all items served from journal cache (no dispatch) ──────────

    #[test]
    fn all_items_cached() {
        let (lua, cx, _rt, journal, _dir) = make_cx_with_journal(vec![MockBehavior::Success {
            output: serde_json::json!({}),
            tokens: TokenUsage::default(),
            delay: Duration::ZERO,
        }]);
        register(&lua, &cx).unwrap();

        // Pre-populate cache with the key that the Lua map fn will produce.
        let opts = lua.create_table().unwrap();
        opts.set("prompt", "cached_prompt").unwrap();
        let (_, cache_key, _) = build_task(&opts, 0, &Arc::new(AtomicU32::new(0))).unwrap();
        journal
            .cache_agent(
                &cache_key,
                Uuid::now_v7(),
                0,
                AgentStatus::Ok,
                serde_json::json!({ "from_cache": true }),
                vec![],
                TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                },
            )
            .unwrap();

        let script = r#"
            return parallel(
                { 1, 2, 3 },
                function()
                    return { prompt = "cached_prompt" }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 3);
        for i in 1..=3 {
            let r: mlua::Table = results.get(i).unwrap();
            assert_eq!(r.get::<String>("status").unwrap(), "ok");
            assert!(r.get::<bool>("ok").unwrap());
        }
    }

    // ── mix of cached and fresh items ──────────────────────────────

    #[test]
    fn partial_cache_mixed() {
        let (lua, cx, _rt, journal, _dir, backend) = make_cx_with_journal_and_backend(vec![
            // One non-cached item will be dispatched
            MockBehavior::Success {
                output: serde_json::json!({ "fresh": true }),
                tokens: TokenUsage {
                    input: 5,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::ZERO,
            },
        ]);
        register(&lua, &cx).unwrap();

        // Cache "aaa" prompt
        let opts1 = lua.create_table().unwrap();
        opts1.set("prompt", "aaa").unwrap();
        let (_, ck1, _) = build_task(&opts1, 0, &Arc::new(AtomicU32::new(0))).unwrap();
        journal
            .cache_agent(
                &ck1,
                Uuid::now_v7(),
                0,
                AgentStatus::Ok,
                serde_json::json!({ "source": "cache" }),
                vec![],
                TokenUsage::default(),
            )
            .unwrap();

        let script = r#"
            return parallel(
                { "first", "second", "third" },
                function(item)
                    if item == "second" then
                        return { prompt = "bbb" }
                    else
                        return { prompt = "aaa" }
                    end
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 3);

        // Only "second" should have been dispatched (bbb is not cached)
        assert_eq!(backend.call_count(), 1, "expected exactly 1 backend call");

        // "first" and "third" should come from cache with { source = "cache" }
        // "second" should come from fresh run with { fresh = true }
        for i in 1..=3 {
            let r: mlua::Table = results.get(i).unwrap();
            assert_eq!(r.get::<String>("status").unwrap(), "ok");
            assert!(r.get::<bool>("ok").unwrap());
            let output: mlua::Table = r.get("output").unwrap();

            // Items 1 and 3 are cached, item 2 is fresh
            if i == 2 {
                assert!(
                    output.get::<bool>("fresh").unwrap_or(false),
                    "item {} should come from fresh run",
                    i,
                );
            } else {
                assert_eq!(
                    output.get::<String>("source").unwrap(),
                    "cache",
                    "item {} should come from cache",
                    i,
                );
            }
        }
    }
}
