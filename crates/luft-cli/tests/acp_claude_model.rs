//! Integration test: `AcpAdapter` sets the requested model via
//! `session/set_config_option` when the ACP agent advertises a `model`
//! config option (category `model`, ungrouped select) — the shape used by
//! the official `claude-code-acp` agent (see `docs/architecture/adapters.md`).
//!
//! Uses the `fake_acp` binary (not a real `claude-code-acp` install) so this
//! runs in CI without network access or an `ANTHROPIC_API_KEY`. It proves the
//! wiring — `AcpConfig::model` -> `validate_and_set_model` ->
//! `session/set_config_option` -> agent applies it — independent of whether
//! `claude-code-acp` itself is installed.

use luft::adapters::{AcpAdapter, AcpConfig};
use luft::core::contract::backend::{AgentBackend, AgentTask, RunContext};
use std::path::PathBuf;
use std::time::Duration;

fn fake_acp_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_acp"))
}

fn test_task() -> AgentTask {
    AgentTask {
        agent_id: uuid::Uuid::now_v7(),
        phase_id: 0,
        prompt: "reply with OK".into(),
        model: None,
        allowlist: None,
        workdir: PathBuf::from("."),
        mcp_endpoint: None,
        timeout: Some(Duration::from_secs(30)),
        output_schema: None,
        workdir_override: None,
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
#[serial_test::serial(fake_acp_model_env)]
async fn acp_adapter_sets_model_via_config_option() {
    let model_out = tempfile::NamedTempFile::new().unwrap();
    let model_out_path = model_out.path().to_string_lossy().into_owned();

    std::env::set_var("FAKE_ACP_MODELS", "model-a,model-b");
    std::env::set_var("FAKE_ACP_MODEL_OUT", &model_out_path);

    let mut env_passthrough: Vec<String> = AcpConfig::DEFAULT_ENV_PASSTHROUGH
        .iter()
        .map(|s| s.to_string())
        .collect();
    env_passthrough.push("FAKE_ACP_MODELS".into());
    env_passthrough.push("FAKE_ACP_MODEL_OUT".into());

    let config = AcpConfig {
        id: "claude-acp",
        binary: fake_acp_binary(),
        acp_args: vec![],
        log_level: None,
        connect_timeout: Duration::from_secs(10),
        emit_raw_events: true,
        env_passthrough,
        model: Some("model-b".to_string()),
    };
    let adapter = AcpAdapter::new(config);

    let result = adapter.run(test_task(), test_context()).await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");

    let written = std::fs::read_to_string(&model_out_path).unwrap();
    assert_eq!(
        written, "model-b",
        "AcpAdapter should have sent session/set_config_option(model, \"model-b\")"
    );

    std::env::remove_var("FAKE_ACP_MODELS");
    std::env::remove_var("FAKE_ACP_MODEL_OUT");
}

#[tokio::test]
#[serial_test::serial(fake_acp_model_env)]
async fn acp_adapter_skips_unavailable_model_without_error() {
    let model_out = tempfile::NamedTempFile::new().unwrap();
    let model_out_path = model_out.path().to_string_lossy().into_owned();
    // Pre-seed the file so we can tell whether set_config_option ran at all.
    std::fs::write(&model_out_path, "untouched").unwrap();

    std::env::set_var("FAKE_ACP_MODELS", "model-a,model-b");
    std::env::set_var("FAKE_ACP_MODEL_OUT", &model_out_path);

    let mut env_passthrough: Vec<String> = AcpConfig::DEFAULT_ENV_PASSTHROUGH
        .iter()
        .map(|s| s.to_string())
        .collect();
    env_passthrough.push("FAKE_ACP_MODELS".into());
    env_passthrough.push("FAKE_ACP_MODEL_OUT".into());

    let config = AcpConfig {
        id: "claude-acp",
        binary: fake_acp_binary(),
        acp_args: vec![],
        log_level: None,
        connect_timeout: Duration::from_secs(10),
        emit_raw_events: true,
        env_passthrough,
        // Not in the advertised "model-a,model-b" list: validate_and_set_model
        // should log a warning and fall back to the agent default, not fail.
        model: Some("model-does-not-exist".to_string()),
    };
    let adapter = AcpAdapter::new(config);

    let result = adapter.run(test_task(), test_context()).await;
    assert!(
        result.is_ok(),
        "an unavailable model must not fail the run: {result:?}"
    );

    let written = std::fs::read_to_string(&model_out_path).unwrap();
    assert_eq!(
        written, "untouched",
        "set_config_option must not be sent for an unavailable model"
    );

    std::env::remove_var("FAKE_ACP_MODELS");
    std::env::remove_var("FAKE_ACP_MODEL_OUT");
}
