//! Integration test: `AcpAdapter` extracts structured_output from a fake ACP agent.
//!
//! This test lives in `tests/` because it needs the `fake-acp` binary target,
//! whose path is exposed via `CARGO_BIN_EXE_fake-acp` only for integration tests.

use luft::adapters::{AcpAdapter, AcpConfig};
use luft::core::contract::backend::{AgentBackend, AgentTask, RunContext};
use std::path::PathBuf;
use std::time::Duration;

fn fake_acp_env() -> Vec<String> {
    let mut env: Vec<String> = AcpConfig::DEFAULT_ENV_PASSTHROUGH
        .iter()
        .map(|s| s.to_string())
        .collect();
    env.push("FAKE_ACP_RAW_INPUT".into());
    env
}

fn fake_acp_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_acp"))
}

fn test_task(timeout_secs: u64, output_schema: Option<serde_json::Value>) -> AgentTask {
    AgentTask {
        agent_id: uuid::Uuid::now_v7(),
        phase_id: 0,
        prompt: "analyze result_collector.rs".into(),
        model: None,
        allowlist: None,
        workdir: PathBuf::from("."),
        mcp_endpoint: None,
        timeout: Some(Duration::from_secs(timeout_secs)),
        output_schema,
        description: None,
        role: None,
        name: None,
        agent_seq: 0,
    }
}

fn test_context() -> RunContext {
    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    RunContext {
        run_id: uuid::Uuid::now_v7(),
        cancel: tokio_util::sync::CancellationToken::new(),
        events: tx,
    }
}

#[tokio::test]
async fn acp_adapter_extracts_structured_output_from_tool_call() {
    let expected = serde_json::json!({
        "file": "src/adapters/result_collector.rs",
        "kind": "rust",
        "summary": "collects agent results"
    });

    std::env::set_var("FAKE_ACP_RAW_INPUT", expected.to_string());

    let config = AcpConfig {
        id: "fake-acp",
        binary: fake_acp_binary(),
        acp_args: vec![],
        log_level: None,
        connect_timeout: Duration::from_secs(10),
        emit_raw_events: true,
        env_passthrough: fake_acp_env(),
        model: None,
    };
    let adapter = AcpAdapter::new(config);

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "file": {"type": "string"},
            "kind": {"type": "string"},
            "summary": {"type": "string"}
        },
        "required": ["file", "kind", "summary"]
    });

    let task = test_task(30, Some(schema));
    let ctx = test_context();

    let result = adapter.run(task, ctx).await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
    assert_eq!(result.unwrap().output, expected);

    std::env::remove_var("FAKE_ACP_RAW_INPUT");
}
