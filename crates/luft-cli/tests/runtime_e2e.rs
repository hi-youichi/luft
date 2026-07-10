//! End-to-end runtime tests: drive Lua workflows through the real scheduler
//! with the deterministic `MockBackend`, exercising the wired SDK primitives
//! (`agent`, `parallel`, `pipeline`, `converge`, `workflow`) and resume replay.

use luft::core::contract::backend::{AgentBackend, RunContext};
use luft::core::{
    AgentCacheKey, BackendRegistry, JournalStore, MockBackend, MockBehavior, Scheduler,
    SchedulerConfig, TokenUsage,
};
use luft::runtime::{ExecLimits, Runtime};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn ok_backend() -> Arc<MockBackend> {
    Arc::new(MockBackend::new(
        "mock",
        vec![MockBehavior::Success {
            output: serde_json::json!({ "v": 1 }),
            tokens: TokenUsage {
                input: 3,
                output: 2,
                cache_read: 0,
                cache_write: 0,
            },
            delay: Duration::from_millis(1),
        }],
    ))
}

/// Build a runtime around `backend` and run `script`, returning the report.
async fn run_with(
    backend: Arc<dyn AgentBackend>,
    script: &str,
    journal: Option<Arc<JournalStore>>,
    run_id: uuid::Uuid,
) -> serde_json::Value {
    let registry = BackendRegistry::new().with(backend);
    let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
    let (tx, _rx) = tokio::sync::broadcast::channel(256);
    let run_ctx = RunContext {
        run_id,
        cancel: CancellationToken::new(),
        events: tx,
    };
    scheduler.init_run_with(run_id, run_ctx.events.clone());

    let handle = tokio::runtime::Handle::current();
    let rt = Runtime::new(
        scheduler,
        run_ctx,
        serde_json::json!({}),
        ExecLimits::default(),
        journal,
        handle,
    )
    .expect("runtime init");

    let s = script.to_string();
    // SDK primitives call Handle::block_on internally, so execute off the async
    // worker threads (mirrors cli::run).
    tokio::task::spawn_blocking(move || rt.execute(&s))
        .await
        .expect("join")
        .expect("script ok")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_parallel_pipeline_converge() {
    let script = r#"
        function main()
            local p = phase("work", 3)
            log("phase id " .. p)
            local a = agent({ prompt = "single", model = "mock" })
            local r = parallel({"x", "y", "z"}, function(it)
                return { prompt = "review " .. it, model = "mock" }
            end)
            local pl = pipeline({
                items = {"a", "b"},
                stages = {
                    function(x) return { step1 = x } end,
                    { label = "two", handler = function(x) return { step2 = x } end },
                },
            })
            report({
                a_ok = a.ok,
                par_n = #r,
                par_all = r[1].ok and r[2].ok and r[3].ok,
                pl_ok = pl.ok,
                pl_failed = pl.failed,
            })
        end
    "#;
    let out = run_with(ok_backend(), script, None, uuid::Uuid::now_v7()).await;
    assert_eq!(out["a_ok"], true);
    assert_eq!(out["par_n"], 3);
    assert_eq!(out["par_all"], true);
    assert_eq!(out["pl_ok"], 2);
    assert_eq!(out["pl_failed"], 0);
}

/// Run `script` and return every event that landed on the bus (drained after
/// execution). Mirrors `run_with` but keeps the receiver alive.
async fn run_collecting_events(
    backend: Arc<dyn AgentBackend>,
    script: &str,
) -> Vec<luft::core::contract::event::AgentEvent> {
    let registry = BackendRegistry::new().with(backend);
    let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
    let run_id = uuid::Uuid::now_v7();
    let (tx, mut rx) = tokio::sync::broadcast::channel(2048);
    let run_ctx = RunContext {
        run_id,
        cancel: CancellationToken::new(),
        events: tx,
    };
    scheduler.init_run_with(run_id, run_ctx.events.clone());
    let handle = tokio::runtime::Handle::current();
    let rt = Runtime::new(
        scheduler,
        run_ctx,
        serde_json::json!({}),
        ExecLimits::default(),
        None,
        handle,
    )
    .expect("runtime init");

    let s = script.to_string();
    tokio::task::spawn_blocking(move || rt.execute(&s))
        .await
        .expect("join")
        .expect("script ok");

    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    events
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_events_emitted_with_span_pairing() {
    use luft::core::contract::event::AgentEvent;

    let script = r#"
        function main()
            budget(300000, 5)
            phase("work", 3)
            local r = parallel({"x","y","z"}, function(it)
                return { prompt = "review " .. it, model = "mock" }
            end)
            report({ n = #r })
        end
    "#;
    let events = run_collecting_events(ok_backend(), script).await;

    // budget(t, r) → BudgetSet with the literal values.
    let budget = events.iter().find_map(|e| match e {
        AgentEvent::BudgetSet {
            time_limit_ms,
            max_rounds,
            ..
        } => Some((*time_limit_ms, *max_rounds)),
        _ => None,
    });
    assert_eq!(budget, Some((Some(300000), Some(5))));

    // parallel → Started/Done sharing one span_id; 3 items all succeed under mock.
    let p_start = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ParallelStarted { span_id, count, .. } => Some((*span_id, *count)),
            _ => None,
        })
        .expect("ParallelStarted");
    let p_done = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ParallelDone {
                span_id,
                ok,
                failed,
                ..
            } => Some((*span_id, *ok, *failed)),
            _ => None,
        })
        .expect("ParallelDone");
    assert_eq!(p_start.0, p_done.0, "parallel span_id must pair");
    assert_eq!(p_start.1, 3);
    assert_eq!((p_done.1, p_done.2), (3, 0));

    // report → ReportEmitted carrying the full value.
    let report = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ReportEmitted { report, .. } => Some(report.clone()),
            _ => None,
        })
        .expect("ReportEmitted");
    assert_eq!(report["n"], 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_events_emitted() {
    use luft::core::contract::event::AgentEvent;

    let dir = tempfile::tempdir().unwrap();
    let sub_path = dir.path().join("sub.lua");
    std::fs::write(&sub_path, "function main() report({ sub = true }) end").unwrap();

    let script = format!(
        r#"
        function main()
            local w = workflow("{}", {{ k = 1 }})
            report({{ got = w.sub }})
        end
    "#,
        sub_path.display().to_string().replace('\\', "\\\\")
    );
    let events = run_collecting_events(ok_backend(), &script).await;

    let w_start = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::WorkflowStarted {
                span_id,
                path,
                args,
                ..
            } => Some((*span_id, path.clone(), args.clone())),
            _ => None,
        })
        .expect("WorkflowStarted");
    let w_done = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::WorkflowDone {
                span_id,
                report,
                error,
                ..
            } => Some((*span_id, report.clone(), error.clone())),
            _ => None,
        })
        .expect("WorkflowDone");

    assert_eq!(w_start.0, w_done.0, "workflow span_id must pair");
    assert!(w_start.1.ends_with("sub.lua"));
    assert_eq!(w_start.2["k"], 1);
    assert!(w_done.2.is_none(), "sub-workflow succeeded");
    assert_eq!(w_done.1["sub"], true, "WorkflowDone carries the sub-report");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_skips_cached_agent() {
    let dir = tempfile::tempdir().unwrap();
    let run_id = uuid::Uuid::now_v7();
    let journal = Arc::new(JournalStore::new(dir.path()).unwrap());
    journal.init_run(run_id, "resume test").unwrap();

    let backend = ok_backend();
    let script = r#"
        function main()
            local a = agent({ prompt = "expensive", model = "mock" })
            report({ ok = a.ok })
        end
    "#;

    // First run executes the agent once and caches it.
    let out1 = run_with(backend.clone(), script, Some(journal.clone()), run_id).await;
    assert_eq!(out1["ok"], true);
    assert_eq!(backend.call_count(), 1);
    assert!(journal.has_completed(&AgentCacheKey::new("expensive", Some("mock"), 0)));

    // Second run with the same journal must replay from cache, not re-invoke.
    let out2 = run_with(backend.clone(), script, Some(journal.clone()), run_id).await;
    assert_eq!(out2["ok"], true);
    assert_eq!(backend.call_count(), 1, "cached agent must not run again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schema_validation_rejects_bad_output() {
    // Mock returns an object without the required field → validation fails →
    // agent() raises, so report() is never reached and execute() errors.
    let backend = ok_backend();
    let registry = BackendRegistry::new().with(backend as Arc<dyn AgentBackend>);
    let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
    let run_id = uuid::Uuid::now_v7();
    let (tx, _rx) = tokio::sync::broadcast::channel(64);
    let run_ctx = RunContext {
        run_id,
        cancel: CancellationToken::new(),
        events: tx,
    };
    scheduler.init_run_with(run_id, run_ctx.events.clone());
    let handle = tokio::runtime::Handle::current();
    let rt = Runtime::new(
        scheduler,
        run_ctx,
        serde_json::json!({}),
        ExecLimits::default(),
        None,
        handle,
    )
    .unwrap();

    let script = r#"
        function main()
            agent({ prompt = "x", model = "mock", schema = { type = "object", required = {"missing"} } })
            report({ unreachable = true })
        end
    "#
    .to_string();
    let res = tokio::task::spawn_blocking(move || rt.execute(&script))
        .await
        .unwrap();
    assert!(
        res.is_err(),
        "schema mismatch should surface as a script error"
    );
}
