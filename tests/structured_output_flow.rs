//! End-to-end test for the `structured_output` MCP tool flow.
//!
//! This drives the real scheduler + real `AcpAdapter` + fake ACP agent binary
//! (`fake-acp`) through the Lua runtime. It verifies that:
//!
//! 1. The SDK injects the "call structured_output tool" instruction.
//! 2. `AcpAdapter` spawns the MCP `maestro mcp-structured-output` server.
//! 3. The fake agent emits a `session/update` ToolCall.
//! 4. `update_mapper` captures the tool raw input as the agent result.
//! 5. The scheduler returns the schema-valid output to the workflow.

use maestro::adapters::{AcpAdapter, AcpConfig};
use maestro::core::contract::backend::{AgentBackend, RunContext};
use maestro::core::{BackendRegistry, Scheduler, SchedulerConfig};
use maestro::runtime::{ExecLimits, Runtime};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn fake_acp_backend() -> Arc<dyn AgentBackend> {
    let mut env_passthrough: Vec<String> = AcpConfig::DEFAULT_ENV_PASSTHROUGH
        .iter()
        .map(|s| s.to_string())
        .collect();
    // The fake ACP binary reads the desired raw tool input from this env var.
    env_passthrough.push("FAKE_ACP_RAW_INPUT".into());

    let binary = std::env::var("CARGO_BIN_EXE_fake_acp")
        .or_else(|_| std::env::var("CARGO_BIN_EXE_fake-acp"))
        .expect("CARGO_BIN_EXE_fake_acp not set; fake-acp binary must be built")
        .into();

    let config = AcpConfig {
        id: "fake-acp",
        binary,
        acp_args: vec![],
        log_level: None,
        connect_timeout: Duration::from_secs(10),
        emit_raw_events: true,
        env_passthrough,
    };
    Arc::new(AcpAdapter::new(config))
}

async fn run_with_fake_acp(
    schema: serde_json::Value,
    raw_input: serde_json::Value,
) -> serde_json::Value {
    std::env::set_var("FAKE_ACP_RAW_INPUT", raw_input.to_string());

    let backend = fake_acp_backend();
    let registry = BackendRegistry::new().with(backend);
    let scheduler = Scheduler::new(SchedulerConfig::default(), registry, None);
    let (tx, _rx) = tokio::sync::broadcast::channel(256);
    let run_id = uuid::Uuid::now_v7();
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

    let schema_json = serde_json::to_string(&schema).unwrap();
    let script = format!(
        r#"
        local result = agent({{
            prompt = "analyze result_collector.rs",
            model = "fake-acp",
            backend = "fake-acp",
            timeout_ms = 10000,
            schema = {schema_json}
        }})
        report({{
            ok = result.ok,
            status = result.status,
            output = result.output
        }})
    "#
    );

    let result = tokio::task::spawn_blocking(move || rt.execute(&script))
        .await
        .expect("join")
        .expect("script ok");

    std::env::remove_var("FAKE_ACP_RAW_INPUT");
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_structured_output_tool_call_returns_valid_schema_data() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "file": {"type": "string"},
            "kind": {"type": "string"},
            "summary": {"type": "string"}
        },
        "required": ["file", "kind", "summary"]
    });

    let raw_input = serde_json::json!({
        "file": "src/adapters/result_collector.rs",
        "kind": "rust",
        "summary": "collects agent results"
    });

    let report = run_with_fake_acp(schema, raw_input.clone()).await;

    assert_eq!(report["ok"], true, "report: {report}");
    assert_eq!(report["status"], "ok");
    assert_eq!(report["output"], raw_input);
}
