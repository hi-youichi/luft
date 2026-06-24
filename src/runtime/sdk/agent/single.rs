//! `agent(opts)` — run a single agent through the scheduler.
//!
//! On a journal cache hit it emits a resume log and returns the cached result
//! without re-running; otherwise it blocks on the scheduler and records the
//! outcome back into the journal.

use super::journal::{record, slot_from_cache, slot_from_result};
use crate::core::contract::event::{AgentEvent, LogLevel};
use crate::runtime::sdk::task::{build_result_table, build_task};
use crate::runtime::sdk::SdkContext;
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
                let (status, output, tokens, findings) = slot_from_cache(cached);
                return build_result_table(lua, &status, output, tokens, &findings);
            }
        }

        let agent_id = task.agent_id;
        tracing::debug!(%agent_id, backend = ?backend, "agent() submitting to scheduler");
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
    })?;
    globals.set("agent", agent_fn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::register;
    use crate::core::contract::backend::{AgentStatus, RunContext};
    use crate::core::contract::ids::TokenUsage;
    use crate::core::journal::JournalStore;
    use crate::core::scheduler::{BackendRegistry, SchedulerConfig};
    use crate::core::Scheduler;
    use crate::core::{MockBackend, MockBehavior, FailKind};
    use crate::runtime::sdk::task::build_task;
    use crate::runtime::sdk::ReportSink;
    use crate::runtime::sdk::SdkContext;
    use mlua::Lua;
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
                .with(backend as Arc<dyn crate::core::contract::backend::AgentBackend>),
            None,
        );
        scheduler.init_run_with(run_id, run_ctx.events.clone());
        let cx = SdkContext::new(run_ctx, scheduler, report_sink, None, handle);
        (lua, cx, rt)
    }

    fn make_cx_with_journal(
        behaviors: Vec<MockBehavior>,
    ) -> (Lua, SdkContext, tokio::runtime::Runtime, Arc<JournalStore>, tempfile::TempDir) {
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
                .with(backend as Arc<dyn crate::core::contract::backend::AgentBackend>),
            None,
        );
        scheduler.init_run_with(run_id, run_ctx.events.clone());

        let dir = tempfile::TempDir::new().unwrap();
        let journal = Arc::new(JournalStore::new(dir.path()).unwrap());
        journal.init_run(run_id, "test").unwrap();

        let cx =
            SdkContext::new(run_ctx, scheduler, report_sink, Some(journal.clone()), handle);
        (lua, cx, rt, journal, dir)
    }

    #[test]
    fn agent_success() {
        let (lua, cx, _rt) = make_cx(vec![MockBehavior::Success {
            output: serde_json::json!({ "result": "hello" }),
            tokens: TokenUsage {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
            },
            delay: Duration::ZERO,
        }]);
        register(&lua, &cx).unwrap();

        let result: mlua::Table = lua
            .load(r#"return agent({ prompt = "test prompt" })"#)
            .eval()
            .unwrap();

        assert_eq!(result.get::<String>("status").unwrap(), "ok");
        assert!(result.get::<bool>("ok").unwrap());
        assert_eq!(result.get::<i64>("tokens").unwrap(), 15);
    }

    #[test]
    fn agent_scheduler_error() {
        let (lua, cx, _rt) = make_cx(vec![MockBehavior::Fail {
            kind: FailKind::Spawn,
            delay: Duration::ZERO,
        }]);
        register(&lua, &cx).unwrap();

        let err = lua
            .load(r#"agent({ prompt = "test prompt" })"#)
            .eval::<mlua::Value>()
            .unwrap_err();

        assert!(err.to_string().contains("agent error:"));
    }

    #[test]
    fn agent_missing_prompt() {
        let (lua, cx, _rt) = make_cx(vec![MockBehavior::Success {
            output: serde_json::json!({}),
            tokens: TokenUsage::default(),
            delay: Duration::ZERO,
        }]);
        register(&lua, &cx).unwrap();

        let err = lua
            .load(r#"agent({})"#)
            .eval::<mlua::Value>()
            .unwrap_err();

        assert!(err.to_string().contains("missing required 'prompt'"));
    }

    #[test]
    fn agent_cache_hit() {
        let (lua, cx, _rt, journal, _dir) = make_cx_with_journal(vec![MockBehavior::Success {
            output: serde_json::json!({}),
            tokens: TokenUsage::default(),
            delay: Duration::ZERO,
        }]);

        let opts = lua.create_table().unwrap();
        opts.set("prompt", "test prompt").unwrap();
        let (_task, cache_key, _backend) = build_task(&opts, 0).unwrap();

        journal
            .cache_agent(
                &cache_key,
                Uuid::now_v7(),
                0,
                AgentStatus::Ok,
                serde_json::json!({ "cached": "yes" }),
                vec![],
                TokenUsage {
                    input: 5,
                    output: 3,
                    cache_read: 0,
                    cache_write: 0,
                },
            )
            .unwrap();

        register(&lua, &cx).unwrap();

        let result: mlua::Table = lua
            .load(r#"return agent({ prompt = "test prompt" })"#)
            .eval()
            .unwrap();

        assert_eq!(result.get::<String>("status").unwrap(), "ok");
        assert!(result.get::<bool>("ok").unwrap());
        assert_eq!(result.get::<i64>("tokens").unwrap(), 8);
    }

    #[test]
    fn agent_cache_miss() {
        let (lua, cx, _rt, _journal, _dir) = make_cx_with_journal(vec![MockBehavior::Success {
            output: serde_json::json!({ "fresh": "result" }),
            tokens: TokenUsage {
                input: 7,
                output: 2,
                cache_read: 0,
                cache_write: 0,
            },
            delay: Duration::ZERO,
        }]);

        register(&lua, &cx).unwrap();

        let result: mlua::Table = lua
            .load(r#"return agent({ prompt = "different prompt" })"#)
            .eval()
            .unwrap();

        assert_eq!(result.get::<String>("status").unwrap(), "ok");
        assert_eq!(result.get::<i64>("tokens").unwrap(), 9);
    }
}
