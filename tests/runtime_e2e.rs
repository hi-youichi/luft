//! End-to-end runtime tests: drive Lua workflows through the real scheduler
//! with the deterministic `MockBackend`, exercising the wired SDK primitives
//! (`agent`, `parallel`, `pipeline`, `converge`, `workflow`) and resume replay.

use maestro::core::contract::backend::{AgentBackend, RunContext};
use maestro::core::{
    AgentCacheKey, BackendRegistry, JournalStore, MockBackend, MockBehavior, Scheduler,
    SchedulerConfig, TokenUsage,
};
use maestro::runtime::{ExecLimits, Runtime};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn ok_backend() -> Arc<MockBackend> {
    Arc::new(MockBackend::new(
        "mock",
        vec![MockBehavior::Success {
            output: serde_json::json!({ "v": 1 }),
            tokens: TokenUsage { input: 3, output: 2, cache_read: 0, cache_write: 0 },
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
    let run_ctx = RunContext { run_id, cancel: CancellationToken::new(), events: tx };
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
        local cv = converge({"c1", "c2"}, { max_rounds = 2 })
        report({
            a_ok = a.ok,
            par_n = #r,
            par_all = r[1].ok and r[2].ok and r[3].ok,
            pl_ok = pl.ok,
            pl_failed = pl.failed,
            cv_rounds = cv.rounds,
        })
    "#;
    let out = run_with(ok_backend(), script, None, uuid::Uuid::now_v7()).await;
    assert_eq!(out["a_ok"], true);
    assert_eq!(out["par_n"], 3);
    assert_eq!(out["par_all"], true);
    assert_eq!(out["pl_ok"], 2);
    assert_eq!(out["pl_failed"], 0);
    // Mock emits no findings, so convergence terminates immediately.
    assert_eq!(out["cv_rounds"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_skips_cached_agent() {
    let dir = tempfile::tempdir().unwrap();
    let run_id = uuid::Uuid::now_v7();
    let journal = Arc::new(JournalStore::new(dir.path()).unwrap());
    journal.init_run(run_id, "resume test").unwrap();

    let backend = ok_backend();
    let script = r#"
        local a = agent({ prompt = "expensive", model = "mock" })
        report({ ok = a.ok })
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
    let run_ctx = RunContext { run_id, cancel: CancellationToken::new(), events: tx };
    scheduler.init_run_with(run_id, run_ctx.events.clone());
    let handle = tokio::runtime::Handle::current();
    let rt = Runtime::new(scheduler, run_ctx, serde_json::json!({}), ExecLimits::default(), None, handle)
        .unwrap();

    let script = r#"
        agent({ prompt = "x", model = "mock", schema = { type = "object", required = {"missing"} } })
        report({ unreachable = true })
    "#
    .to_string();
    let res = tokio::task::spawn_blocking(move || rt.execute(&script)).await.unwrap();
    assert!(res.is_err(), "schema mismatch should surface as a script error");
}
