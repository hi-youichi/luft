//! `parallel(items, mapFn)` — barrier fan-out over the scheduler.
//!
//! `mapFn(item) -> opts` produces a task per item; all non-cached tasks run
//! concurrently under the scheduler's global semaphore and results are returned
//! in input order. Cached items (resume) are filled in without re-running.

use super::journal::{record, slot_from_cache, slot_from_result, Slot};
use crate::core::contract::backend::AgentTask;
use crate::core::contract::event::AgentEvent;
use crate::core::contract::ids::AgentId;
use crate::core::journal::AgentCacheKey;
use crate::runtime::sdk::task::{build_result_table, build_task};
use crate::runtime::sdk::SdkContext;
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
                let (task, cache_key, backend) = build_task(&opts, phase_id)?;

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

                for (p, res) in pending.iter().zip(results.into_iter()) {
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
