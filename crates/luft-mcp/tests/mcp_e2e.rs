//! E2E test: drive a workflow through the service layer (the same API the MCP
//! tools call) with the mock backend. Exercises the full
//! service → runtime → scheduler → agent path.

use luft_core::{MockBackend, MockBehavior, TokenUsage};
use luft_mcp::LuftMcpServer;
use luft_service::request::{
    CancelRunRequest, ExecuteWorkflowRequest, GetRunEventsRequest, GetRunStatusRequest,
    ListRunsRequest,
};
use luft_service::WorkflowService;
use std::time::Duration;

fn make_server(behaviors: Vec<MockBehavior>) -> LuftMcpServer {
    let backend = MockBackend::new("mock", behaviors);
    let luft = luft::Luft::builder()
        .backend(backend)
        .base_dir(tempfile::TempDir::new().unwrap().keep())
        .build()
        .unwrap();
    LuftMcpServer::new(luft)
}

fn ok_behaviors(n: usize) -> Vec<MockBehavior> {
    std::iter::repeat_n(
        MockBehavior::Success {
            output: serde_json::json!({"analysis": "done"}),
            tokens: TokenUsage {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
            },
            delay: Duration::from_millis(1),
        },
        n,
    )
    .collect()
}

// ── Test: single-agent workflow via start → join → status ────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_single_agent_workflow() {
    let server = make_server(ok_behaviors(1));

    let script = r#"
        meta = {
            reasoning = "Single agent test",
            phases = { { label = "analyze" } },
        }
        function main()
            phase("analyze")
            local r = agent({ prompt = "analyze the data", model = "mock" })
            report({
                summary = "agent completed",
                ok = r.ok,
                output = r.output,
            })
        end
    "#;

    let (exec, handle) = server
        .service
        .start_workflow(ExecuteWorkflowRequest {
            script: Some(script.to_string()),
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: Some(1),
            backend: None,
        })
        .await
        .unwrap();
    assert_eq!(exec.status, "running");

    let outcome = handle.join().await.unwrap();
    let report = outcome.result.unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["summary"], "agent completed");
    assert_eq!(report["output"]["analysis"], "done");

    let status = server
        .service
        .get_run_status(GetRunStatusRequest {
            run_id: outcome.run_dir_name.clone(),
        })
        .unwrap();
    assert!(status.completed_agents > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_execute_cancel_status_and_run_done_cancelled() {
    let server = make_server(vec![MockBehavior::Hang]);
    let script = r#"
        meta = { reasoning = "cancellation", phases = { { label = "work" } } }
        function main()
            phase("work", 1)
            local r = agent({ prompt = "hang", model = "mock" })
            report(r)
        end
    "#;

    let (exec, handle) = server
        .service
        .start_workflow(ExecuteWorkflowRequest {
            script: Some(script.into()),
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: Some(1),
            backend: None,
        })
        .await
        .unwrap();

    let run_id = exec.run_id;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if server
                .service
                .get_run_status(GetRunStatusRequest {
                    run_id: run_id.clone(),
                })
                .is_ok_and(|s| s.status == "running")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("run should become observable");

    let cancel = server
        .service
        .cancel_run(CancelRunRequest {
            run_id: run_id.clone(),
        })
        .unwrap();
    assert_eq!(cancel.result, "cancelling");

    tokio::time::timeout(Duration::from_secs(5), handle.join())
        .await
        .expect("cancelled run should join")
        .expect("run task should not panic");

    let status = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let status = server
                .service
                .get_run_status(GetRunStatusRequest {
                    run_id: run_id.clone(),
                })
                .unwrap();
            if status.status == "cancelled" {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("cancelled status should be persisted");
    assert_eq!(status.status, "cancelled");

    let run_done = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let events = server
                .service
                .get_run_events(GetRunEventsRequest {
                    run_id: run_id.clone(),
                    since_event_id: None,
                    offset: None,
                    events_limit: None,
                    types: Some(vec!["run_done".into()]),
                    agent_id: None,
                })
                .unwrap();
            if events.events.iter().any(|event| {
                event.get("type").and_then(|t| t.as_str()) == Some("run_done")
                    && event.get("status").and_then(|s| s.as_str()) == Some("cancelled")
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(run_done.is_ok(), "cancelled RunDone should be persisted");
}

// ── Test: parallel agents workflow ───────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_parallel_workflow() {
    let server = make_server(ok_behaviors(3));

    let script = r#"
        meta = {
            reasoning = "Parallel review",
            phases = {
                { label = "review", dynamic = true },
                { label = "report" },
            },
        }
        local ITEMS = { "alpha", "beta", "gamma" }

        function main()
            phase("review", #ITEMS)
            local results = parallel(ITEMS, function(item)
                return { prompt = "review: " .. item, model = "mock" }
            end)
            phase("report")

            local ok_count = 0
            for _, r in ipairs(results) do
                if r.ok then ok_count = ok_count + 1 end
            end
            report({
                summary = "parallel done",
                total = #results,
                ok_count = ok_count,
            })
        end
    "#;

    let (_exec, handle) = server
        .service
        .start_workflow(ExecuteWorkflowRequest {
            script: Some(script.to_string()),
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: Some(4),
            backend: None,
        })
        .await
        .unwrap();
    let outcome = handle.join().await.unwrap();

    let report = outcome.result.unwrap();
    assert_eq!(report["total"], 3);
    assert_eq!(report["ok_count"], 3);

    let status = server
        .service
        .get_run_status(GetRunStatusRequest {
            run_id: outcome.run_dir_name.clone(),
        })
        .unwrap();
    assert!(status.completed_agents >= 3);
}

// ── Test: events retrieval after completion ──────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_events_after_completion() {
    let server = make_server(ok_behaviors(1));

    let script = r#"
        meta = {
            reasoning = "Events test",
            phases = { { label = "work" } },
        }
        function main()
            local r = agent({ prompt = "task", model = "mock" })
            report({ ok = r.ok })
        end
    "#;

    let (_exec, handle) = server
        .service
        .start_workflow(ExecuteWorkflowRequest {
            script: Some(script.to_string()),
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: Some(1),
            backend: None,
        })
        .await
        .unwrap();
    let outcome = handle.join().await.unwrap();

    let events = server
        .service
        .get_run_events(GetRunEventsRequest {
            run_id: outcome.run_dir_name.clone(),
            since_event_id: None,
            offset: None,
            events_limit: None,
            types: None,
            agent_id: None,
        })
        .unwrap();
    assert!(!events.events.is_empty());
    assert!(events.total_matching > 0);
}

// ── Test: list_runs includes the run after completion ────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_list_runs_after_completion() {
    let server = make_server(ok_behaviors(1));

    let script = r#"
        meta = {
            reasoning = "List test",
            phases = {},
        }
        function main()
            report({ done = true })
        end
    "#;

    let (_exec, handle) = server
        .service
        .start_workflow(ExecuteWorkflowRequest {
            script: Some(script.to_string()),
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: Some(1),
            backend: None,
        })
        .await
        .unwrap();
    let outcome = handle.join().await.unwrap();

    let list = server
        .service
        .list_runs(ListRunsRequest {
            limit: None,
            cursor: None,
            status_filter: None,
        })
        .unwrap();
    assert!(list.count > 0);
    assert!(
        list.runs
            .iter()
            .any(|r| r.run_id == outcome.run_id.to_string()),
        "run_id {} not found in list: {:?}",
        outcome.run_id,
        list.runs.iter().map(|r| &r.run_id).collect::<Vec<_>>()
    );
}

// ── Test: explicit backend override ───────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_explicit_backend_runs_successfully() {
    let server = make_server(ok_behaviors(1));

    let script = r#"
        meta = { reasoning = "backend test", phases = {} }
        function main()
            local r = agent({ prompt = "hi", model = "mock" })
            report({ ok = r.ok })
        end
    "#;

    let (exec, handle) = server
        .service
        .start_workflow(ExecuteWorkflowRequest {
            script: Some(script.into()),
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: Some(1),
            backend: Some("mock".into()),
        })
        .await
        .unwrap();
    assert_eq!(exec.status, "running");

    let outcome = handle.join().await.unwrap();
    let report = outcome.result.unwrap();
    assert_eq!(report["ok"], true);
}

// ── Test: nonexistent backend returns error with available list ───────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_nonexistent_backend_error_lists_available() {
    let server = make_server(ok_behaviors(1));

    let script = "meta = { reasoning = \"t\", phases = {} } function main() report({}) end";

    let result = server
        .service
        .start_workflow(ExecuteWorkflowRequest {
            script: Some(script.into()),
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: None,
            backend: Some("nonexistent".into()),
        })
        .await;

    let err = result.err().expect("should be an error");
    let msg = err.to_string();
    assert!(msg.contains("nonexistent"), "msg: {msg}");
    assert!(
        msg.contains("mock"),
        "should list available backends, msg: {msg}"
    );
}

// ── Test: no backend field falls back to daemon default ───────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_no_backend_uses_daemon_default() {
    let server = make_server(ok_behaviors(1));

    let script = r#"
        meta = { reasoning = "default backend", phases = {} }
        function main()
            local r = agent({ prompt = "hi", model = "mock" })
            report({ ok = r.ok })
        end
    "#;

    let (_exec, handle) = server
        .service
        .start_workflow(ExecuteWorkflowRequest {
            script: Some(script.into()),
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: Some(1),
            backend: None,
        })
        .await
        .unwrap();
    let outcome = handle.join().await.unwrap();
    assert!(outcome.result.is_ok());
}

// ── Test: empty backend string is rejected ────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_empty_backend_rejected() {
    let server = make_server(ok_behaviors(1));

    let script = "meta = { reasoning = \"t\", phases = {} } function main() report({}) end";

    let result = server
        .service
        .start_workflow(ExecuteWorkflowRequest {
            script: Some(script.into()),
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: None,
            backend: Some("   ".into()),
        })
        .await;

    let err = result.err().expect("empty backend should be rejected");
    assert!(err.to_string().contains("backend"), "msg: {err}");
}
