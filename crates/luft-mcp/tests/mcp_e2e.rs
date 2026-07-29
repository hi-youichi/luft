//! E2E test: drive a workflow through the service layer (the same API the MCP
//! tools call) with the mock backend. Exercises the full
//! service → runtime → scheduler → agent path.

use luft_core::{MockBackend, MockBehavior, TokenUsage};
use luft_mcp::LuftMcpServer;
use luft_service::request::{
    ExecuteWorkflowRequest, GetRunEventsRequest, GetRunStatusRequest, ListRunsRequest,
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
