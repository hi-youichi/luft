//! `pmap(items, mapFn)` — per-item concurrent coroutine map.
//!
//! Unlike `parallel()` (which batches a single agent per item), `pmap()` runs
//! the map function inside a Lua **coroutine** per item. The map function may
//! call `agent()` multiple times (e.g., develop → review → test), and each
//! call yields the task to this driver. The driver dispatches all pending
//! tasks concurrently through the scheduler's semaphore (`--max-concurrency`).
//!
//! ## How it works
//!
//! 1. Create one coroutine per item.
//! 2. Resume all coroutines sequentially. Each runs until its first `agent()`
//!    call, which deposits a task into the [`CoroutineBridge`] and yields a
//!    `request_id`.
//! 3. Main loop: call `block_on(rx.recv())` to get one completed task, resume
//!    its coroutine (sync Lua op). If the coroutine yields again, dispatch the
//!    new task.
//! 4. Repeat until all coroutines are done.
//!
//! The `block_on` calls are interleaved with synchronous Lua manipulation
//! because `&Lua` is not `Send` and cannot enter an async block.

use super::journal::{record, slot_from_result};
use crate::core::contract::event::AgentEvent;
use crate::core::scheduler::SchedulerError;
use crate::runtime::sdk::task::build_result_table;
use crate::runtime::sdk::SdkContext;
use mlua::{Function, Lua, Table, Thread, Value};
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;

/// State tracked per coroutine.
struct CoState {
    thread: Thread,
    item_index: usize,
    done: bool,
}

/// Extract a u64 request_id from a yielded Lua value.
fn extract_req_id(val: &Value) -> Result<u64, mlua::Error> {
    match val {
        Value::Integer(n) => Ok(*n as u64),
        Value::Number(n) => Ok(*n as u64),
        _ => Err(mlua::Error::RuntimeError(format!(
            "pmap: coroutine yielded unexpected value type: {}",
            val.type_name()
        ))),
    }
}

/// Register `pmap` as a Lua global.
pub(super) fn register(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    let globals = lua.globals();
    let run_id = cx.run_id();
    let sched = cx.scheduler.clone();
    let handle = cx.handle.clone();
    let journal = cx.journal.clone();
    let events = cx.events();
    let span_counter = cx.span_counter.clone();
    let bridge = cx.coroutine_bridge.clone();

    let pmap_fn = lua.create_function(move |lua, (items, map_fn): (Table, Function)| {
        let count = items.raw_len();
        let span_id = span_counter.fetch_add(1, Ordering::Relaxed);
        let _ = events.send(AgentEvent::ParallelStarted {
            run_id,
            phase_id: 0,
            span_id,
            count,
        });
        let t0 = std::time::Instant::now();
        tracing::debug!(count, "pmap() fan-out started");

        bridge.enter_pmap();

        let outcome: mlua::Result<Table> = (|| {
            // ── 1. Create the agent-yield wrapper (Lua code) ──
            // agent() in pmap mode returns { __yield = request_id } instead of
            // calling coroutine.yield() directly (can't yield from Rust callback).
            // This Lua wrapper intercepts the sentinel and calls coroutine.yield()
            // at the Lua level, which properly propagates through the coroutine.
            let yield_wrapper: mlua::Function = lua
                .load(
                    r#"
                    return function(real_agent, user_fn)
                        return function(item)
                            local real = real_agent
                            local saved = rawget(_G, 'agent')
                            _G.agent = function(opts)
                                local r = real(opts)
                                local yid = type(r) == 'table' and rawget(r, '__yield')
                                if yid then
                                    return coroutine.yield(yid)
                                end
                                return r
                            end
                            local ok, result = pcall(user_fn, item)
                            _G.agent = saved
                            if not ok then error(result) end
                            return result
                        end
                    end
                    "#,
                )
                .eval::<mlua::Function>()?;

            // ── 2. Create one coroutine per item ──
            let mut co_states: Vec<CoState> = Vec::with_capacity(count);
            let item_values: Vec<Value> = items
                .sequence_values::<Value>()
                .collect::<Result<Vec<_>, _>>()?;

            let real_agent: mlua::Function = lua.globals().get("agent")?;

            for (idx, _item) in item_values.iter().enumerate() {
                let map_fn_ref = map_fn.clone();
                // Wrap the map_fn with the yield-intercepting agent wrapper
                let co_fn: mlua::Function = yield_wrapper
                    .call::<mlua::Function>((real_agent.clone(), map_fn_ref.clone()))?;
                let thread = lua.create_thread(co_fn)?;
                co_states.push(CoState {
                    thread,
                    item_index: idx,
                    done: false,
                });
            }

            // ── 2. Resume all coroutines sequentially (pass item as arg) ──
            let mut results: Vec<Option<Value>> = vec![None; count];
            let (tx, mut rx) = mpsc::channel::<(
                usize,
                std::result::Result<crate::core::contract::backend::AgentResult, SchedulerError>,
            )>(count.max(1) * 4);

            for (co_idx, cs) in co_states.iter_mut().enumerate() {
                let item = item_values[co_idx].clone();
                match cs.thread.resume(item) {
                    Ok(val) => {
                        if cs.thread.status() == mlua::ThreadStatus::Resumable {
                            let req_id = extract_req_id(&val)?;
                            dispatch_task(
                                &bridge, req_id, co_idx, &tx, &sched, &handle, &journal, run_id,
                            );
                        } else {
                            results[cs.item_index] = Some(val);
                            cs.done = true;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(co_idx, error = %e, "pmap: coroutine initial resume error");
                        return Err(e);
                    }
                }
            }

            // ── 3. Main loop: block_on(rx.recv()) → resume → repeat ──
            loop {
                if co_states.iter().all(|cs| cs.done) {
                    break;
                }

                let (co_idx, result) = match handle.block_on(rx.recv()) {
                    Some(v) => v,
                    None => {
                        tracing::warn!("pmap: channel closed before all coroutines done");
                        break;
                    }
                };

                let cs = &mut co_states[co_idx];

                // Build result table for the coroutine
                let result_table = match result {
                    Ok(r) => {
                        let (status, output, tokens, findings) = slot_from_result(r);
                        build_result_table(lua, &status, output, tokens, &findings)?
                    }
                    Err(e) => {
                        tracing::warn!(co_idx, error = %e, "pmap: agent task failed");
                        build_result_table(
                            lua,
                            "error",
                            serde_json::json!({"error": e.to_string()}),
                            0,
                            &[],
                        )?
                    }
                };

                // Resume the coroutine with the result
                match cs.thread.resume(result_table) {
                    Ok(val) => {
                        if cs.thread.status() == mlua::ThreadStatus::Resumable {
                            let req_id = extract_req_id(&val)?;
                            dispatch_task(
                                &bridge, req_id, co_idx, &tx, &sched, &handle, &journal, run_id,
                            );
                        } else {
                            results[cs.item_index] = Some(val);
                            cs.done = true;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(co_idx, error = %e, "pmap: coroutine resume error");
                        return Err(e);
                    }
                }
            }

            // ── 4. Collect results ──
            let arr = lua.create_table()?;
            let mut ok = 0usize;
            let mut failed = 0usize;

            for (i, r) in results.iter().enumerate() {
                let val = r.clone().unwrap_or(Value::Nil);
                let is_ok = match &val {
                    Value::Table(t) => t.get::<bool>("ok").unwrap_or(false),
                    _ => false,
                };
                if is_ok {
                    ok += 1;
                } else {
                    failed += 1;
                }
                arr.set(i + 1, val)?;
            }

            tracing::debug!(ok, failed, "pmap() completed");
            Ok(arr)
        })();

        bridge.exit_pmap();

        let elapsed_ms = t0.elapsed().as_millis() as u64;
        let (ok, failed) = match &outcome {
            Ok(arr) => {
                let mut ok = 0usize;
                let mut failed = 0usize;
                for i in 1..=arr.raw_len() {
                    if let Ok(t) = arr.get::<Table>(i) {
                        if t.get::<bool>("ok").unwrap_or(false) {
                            ok += 1;
                        } else {
                            failed += 1;
                        }
                    }
                }
                (ok, failed)
            }
            Err(_) => (0, count),
        };
        let _ = events.send(AgentEvent::ParallelDone {
            run_id,
            phase_id: 0,
            span_id,
            ok,
            failed,
            results: serde_json::Value::Null,
            elapsed_ms,
        });

        outcome
    })?;

    globals.set("pmap", pmap_fn)?;
    Ok(())
}

/// Retrieve a pending task from the bridge and dispatch it to the scheduler.
#[allow(clippy::too_many_arguments)]
fn dispatch_task(
    bridge: &std::sync::Arc<crate::runtime::sdk::CoroutineBridge>,
    req_id: u64,
    co_idx: usize,
    tx: &mpsc::Sender<(
        usize,
        std::result::Result<crate::core::contract::backend::AgentResult, SchedulerError>,
    )>,
    sched: &std::sync::Arc<crate::core::Scheduler>,
    handle: &tokio::runtime::Handle,
    journal: &Option<std::sync::Arc<crate::core::journal::JournalStore>>,
    run_id: crate::core::contract::ids::RunId,
) {
    let pending = match bridge.take(req_id) {
        Some(p) => p,
        None => {
            tracing::warn!(req_id, "pmap: bridge missing task for request_id");
            return;
        }
    };

    let cache_key = pending.cache_key;
    let agent_id = pending.agent_id;
    let phase_id = pending.phase_id;
    let task = pending.task;
    let backend = pending.backend;
    let tx = tx.clone();
    let sched = sched.clone();
    let journal = journal.clone();

    handle.spawn(async move {
        let result = sched.run_agent(run_id, task, backend.as_deref()).await;
        if let Ok(ref r) = result {
            record(&journal, &cache_key, agent_id, phase_id, r);
        }
        let _ = tx.send((co_idx, result)).await;
    });
}

#[cfg(test)]
mod tests {
    use super::register;
    use crate::core::contract::backend::RunContext;
    use crate::core::contract::ids::TokenUsage;
    use crate::core::scheduler::{BackendRegistry, SchedulerConfig};
    use crate::core::Scheduler;
    use crate::core::{FailKind, MockBackend, MockBehavior};
    use crate::runtime::sdk::agent::single;
    use crate::runtime::sdk::{ReportSink, SdkContext};
    use mlua::Lua;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    /// Build a Lua VM + SdkContext with both `agent` and `pmap` registered.
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
        let bhs = if behaviors.is_empty() {
            vec![MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }]
        } else {
            behaviors
        };
        let backend = Arc::new(MockBackend::new("mock", bhs));
        let scheduler: Arc<Scheduler> = Scheduler::new(
            SchedulerConfig::default(),
            BackendRegistry::new()
                .with(backend as Arc<dyn crate::core::contract::backend::AgentBackend>),
            None,
        );
        scheduler.init_run_with(run_id, run_ctx.events.clone());
        let cx = SdkContext::new(run_ctx, scheduler, report_sink, None, handle);
        single::register(&lua, &cx).unwrap();
        register(&lua, &cx).unwrap();
        (lua, cx, rt)
    }

    // ── empty items ───────────────────────────────────────────

    #[test]
    fn empty_items() {
        let (lua, cx, _rt) = make_cx(vec![]);
        let results: mlua::Table = lua
            .load(r#"pmap({}, function(item) return item end)"#)
            .eval()
            .unwrap();
        assert_eq!(results.raw_len(), 0);
    }

    // ── no agent calls, pure transform ────────────────────────

    #[test]
    fn pure_transform_no_agent() {
        let (lua, cx, _rt) = make_cx(vec![]);
        let script = r#"
            return pmap(
                { "a", "b", "c" },
                function(item) return { name = item, ok = true } end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 3);
        for i in 1..=3 {
            let r: mlua::Table = results.get(i).unwrap();
            assert!(r.get::<bool>("ok").unwrap());
        }
    }

    // ── single agent call per item ────────────────────────────

    #[test]
    fn single_agent_per_item() {
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
        let script = r#"
            return pmap(
                { "x", "y" },
                function(item)
                    local r = agent({ prompt = "task_" .. item })
                    return { name = item, ok = r.ok, output = r.output }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 2);

        let r1: mlua::Table = results.get(1).unwrap();
        assert!(r1.get::<bool>("ok").unwrap());
        let out1: mlua::Table = r1.get("output").unwrap();
        assert_eq!(out1.get::<String>("result").unwrap(), "alpha");

        let r2: mlua::Table = results.get(2).unwrap();
        assert!(r2.get::<bool>("ok").unwrap());
        let out2: mlua::Table = r2.get("output").unwrap();
        assert_eq!(out2.get::<String>("result").unwrap(), "beta");
    }

    // ── multiple agent calls per item (develop→review chain) ──

    #[test]
    fn multi_agent_per_item() {
        let (lua, cx, _rt) = make_cx(vec![
            // doc1: develop
            MockBehavior::Success {
                output: serde_json::json!({ "code": "impl_v1" }),
                tokens: TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::ZERO,
            },
            // doc1: review (approved)
            MockBehavior::Success {
                output: serde_json::json!({ "approved": true }),
                tokens: TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::ZERO,
            },
        ]);
        let script = r#"
            return pmap(
                { "doc1" },
                function(doc)
                    local dev = agent({ prompt = "develop_" .. doc })
                    if not dev.ok then
                        return { doc = doc, ok = false, error = "develop failed" }
                    end
                    local rev = agent({ prompt = "review_" .. doc })
                    if not rev.ok then
                        return { doc = doc, ok = false, error = "review failed" }
                    end
                    return {
                        doc = doc,
                        ok = true,
                        approved = rev.output.approved,
                        impl = dev.output.code
                    }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 1);

        let r: mlua::Table = results.get(1).unwrap();
        assert!(r.get::<bool>("ok").unwrap());
        assert!(r.get::<bool>("approved").unwrap());
        assert_eq!(r.get::<String>("impl").unwrap(), "impl_v1");
    }

    // ── agent failure returns ok=false, no crash ──────────────

    #[test]
    fn agent_failure_isolated() {
        // Both items get the same behavior (success or fail), so order
        // doesn't matter. We test that one success + one fail produces
        // exactly 1 ok and 1 failed result.
        let (lua, cx, _rt) = make_cx(vec![
            // First consumed behavior: success
            MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            },
            // Second consumed behavior: fail
            MockBehavior::Fail {
                kind: FailKind::Protocol,
                delay: Duration::ZERO,
            },
            // Extra behaviors in case order is non-deterministic
            MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            },
            MockBehavior::Fail {
                kind: FailKind::Protocol,
                delay: Duration::ZERO,
            },
        ]);
        let script = r#"
            return pmap(
                { "item1", "item2" },
                function(item)
                    local r = agent({ prompt = "task_" .. item })
                    return { name = item, ok = r.ok }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 2);

        let mut ok_count = 0;
        let mut fail_count = 0;
        for i in 1..=2 {
            let r: mlua::Table = results.get(i).unwrap();
            if r.get::<bool>("ok").unwrap() {
                ok_count += 1;
            } else {
                fail_count += 1;
            }
        }
        assert_eq!(ok_count, 1, "exactly 1 should succeed");
        assert_eq!(fail_count, 1, "exactly 1 should fail");
    }

    // ── all items fail ────────────────────────────────────────

    #[test]
    fn all_items_fail() {
        let (lua, cx, _rt) = make_cx(vec![
            MockBehavior::Fail {
                kind: FailKind::Protocol,
                delay: Duration::ZERO,
            },
            MockBehavior::Fail {
                kind: FailKind::Protocol,
                delay: Duration::ZERO,
            },
            MockBehavior::Fail {
                kind: FailKind::Protocol,
                delay: Duration::ZERO,
            },
        ]);
        let script = r#"
            return pmap(
                { "a", "b", "c" },
                function(item)
                    local r = agent({ prompt = "task_" .. item })
                    return { name = item, ok = r.ok }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 3);
        for i in 1..=3 {
            let r: mlua::Table = results.get(i).unwrap();
            assert!(
                !r.get::<bool>("ok").unwrap(),
                "item {} should have failed",
                i
            );
        }
    }

    // ── map_fn error propagates ───────────────────────────────

    #[test]
    fn map_fn_error_propagates() {
        let (lua, cx, _rt) = make_cx(vec![]);
        let err = lua
            .load(r#"pmap({1, 2}, function() error("boom") end)"#)
            .eval::<mlua::Value>()
            .unwrap_err();
        assert!(
            err.to_string().contains("boom"),
            "unexpected error: {}",
            err
        );
    }

    // ── coroutine yield/resume is transparent to Lua ──────────
    // Verifies that agent() inside pmap() behaves identically to
    // agent() outside pmap() from the Lua script's perspective.

    #[test]
    fn yield_resume_transparent() {
        let (lua, cx, _rt) = make_cx(vec![
            MockBehavior::Success {
                output: serde_json::json!({ "step": 1 }),
                tokens: TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::ZERO,
            },
            MockBehavior::Success {
                output: serde_json::json!({ "step": 2 }),
                tokens: TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::ZERO,
            },
            MockBehavior::Success {
                output: serde_json::json!({ "step": 3 }),
                tokens: TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                },
                delay: Duration::ZERO,
            },
        ]);
        let script = r#"
            return pmap(
                { "doc" },
                function(doc)
                    local results = {}
                    for i = 1, 3 do
                        local r = agent({ prompt = "step_" .. i })
                        table.insert(results, r.output.step)
                    end
                    return { doc = doc, ok = true, steps = results }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 1);

        let r: mlua::Table = results.get(1).unwrap();
        assert!(r.get::<bool>("ok").unwrap());
        let steps: mlua::Table = r.get("steps").unwrap();
        assert_eq!(steps.raw_len(), 3);
        assert_eq!(steps.get::<i64>(1).unwrap(), 1);
        assert_eq!(steps.get::<i64>(2).unwrap(), 2);
        assert_eq!(steps.get::<i64>(3).unwrap(), 3);
    }

    // ── multiple items with multi-step agent chains ───────────
    // This is the real-world pmap use case: multiple docs each
    // running a develop→review loop concurrently.

    #[test]
    fn multi_item_multi_step() {
        // Use identical behaviors for both docs — order non-deterministic
        // with concurrent dispatch, so we avoid relying on specific assignment.
        let mk_dev = || MockBehavior::Success {
            output: serde_json::json!({ "impl": "dev", "approved": true }),
            tokens: TokenUsage::default(),
            delay: Duration::ZERO,
        };
        let mk_rev = || MockBehavior::Success {
            output: serde_json::json!({ "approved": true }),
            tokens: TokenUsage::default(),
            delay: Duration::ZERO,
        };
        // 2 docs × 2 agents = 4 behaviors, but interleaved order may vary.
        // Provide enough behaviors for any ordering.
        let (lua, cx, _rt) = make_cx(vec![mk_dev(), mk_rev(), mk_dev(), mk_rev()]);
        let script = r#"
            return pmap(
                { "doc1", "doc2" },
                function(doc)
                    local dev = agent({ prompt = "dev_" .. doc })
                    if not dev.ok then return { doc = doc, ok = false } end
                    local rev = agent({ prompt = "rev_" .. doc })
                    if not rev.ok then return { doc = doc, ok = false } end
                    return {
                        doc = doc,
                        ok = true,
                        approved = rev.output.approved,
                        impl = dev.output.impl
                    }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 2);

        for i in 1..=2 {
            let r: mlua::Table = results.get(i).unwrap();
            assert!(r.get::<bool>("ok").unwrap(), "item {} should be ok", i);
            assert!(
                r.get::<bool>("approved").unwrap(),
                "item {} should be approved",
                i
            );
        }
    }

    // ── early return (skip remaining agent calls) ────────────
    // If the first agent fails, processDoc returns early without
    // calling the second agent. The unused behaviors should not
    // be consumed.

    #[test]
    fn early_return_on_failure() {
        // Extra behaviors to handle non-deterministic completion order.
        let (lua, cx, _rt) = make_cx(vec![
            MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            },
            MockBehavior::Success {
                output: serde_json::json!({ "approved": true }),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            },
            MockBehavior::Fail {
                kind: FailKind::Protocol,
                delay: Duration::ZERO,
            },
            // Extra: in case completion order varies
            MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            },
            MockBehavior::Success {
                output: serde_json::json!({ "approved": true }),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            },
            MockBehavior::Fail {
                kind: FailKind::Protocol,
                delay: Duration::ZERO,
            },
        ]);
        let script = r#"
            return pmap(
                { "doc1", "doc2" },
                function(doc)
                    local dev = agent({ prompt = "dev_" .. doc })
                    if not dev.ok then
                        return { doc = doc, ok = false, error = "dev failed" }
                    end
                    local rev = agent({ prompt = "rev_" .. doc })
                    return { doc = doc, ok = rev.ok }
                end
            )
        "#;
        let results: mlua::Table = lua.load(script).eval().unwrap();
        assert_eq!(results.raw_len(), 2);

        let mut ok = 0;
        let mut failed = 0;
        for i in 1..=2 {
            let r: mlua::Table = results.get(i).unwrap();
            if r.get::<bool>("ok").unwrap() {
                ok += 1;
            } else {
                failed += 1;
            }
        }
        // At least one should succeed and at least one should fail
        assert!(ok >= 1, "at least 1 should succeed");
        assert!(failed >= 1, "at least 1 should fail");
    }
}
