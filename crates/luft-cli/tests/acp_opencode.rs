//! Live integration tests for the OpenCode ACP backend (P0-A).
//!
//! These are `#[ignore]` by default because they spawn a real `opencode acp`
//! subprocess and require a configured provider. Run with:
//!
//! ```bash
//! cargo test --test acp_opencode -- --ignored --nocapture
//! ```

use luft::adapters::AcpAdapter;
use luft::core::contract::backend::{AgentBackend, AgentStatus, AgentTask, RunContext};
use std::time::Duration;

fn task(prompt: &str, timeout: Duration) -> AgentTask {
    AgentTask {
        agent_id: uuid::Uuid::now_v7(),
        phase_id: 0,
        prompt: prompt.to_string(),
        model: None,
        description: None,
        role: None,
        name: None,
        agent_seq: 0,
        allowlist: None,
        workdir: std::path::PathBuf::from("."),
        mcp_endpoint: None,
        timeout: Some(timeout),
        output_schema: None,
        thread_id: None,
        workdir_override: None,
    }
}

fn ctx() -> (
    RunContext,
    tokio::sync::broadcast::Receiver<luft::core::contract::event::AgentEvent>,
) {
    let (events, rx) = tokio::sync::broadcast::channel(256);
    (
        RunContext {
            run_id: uuid::Uuid::now_v7(),
            cancel: tokio_util::sync::CancellationToken::new(),
            events,
        },
        rx,
    )
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real `opencode` binary + provider config"]
async fn acp_round_trip() {
    let backend = AcpAdapter::default_opencode();
    let (rc, _rx) = ctx();
    let result = backend
        .run(
            task("Reply with exactly: HELLO", Duration::from_secs(120)),
            rc,
        )
        .await
        .expect("opencode run should succeed");

    assert_eq!(result.status, AgentStatus::Ok);
    assert!(
        result.output.get("text").and_then(|t| t.as_str()).is_some() || result.output.is_array(),
        "expected text output or findings, got: {}",
        result.output
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real `opencode` binary"]
async fn acp_cancel_returns_promptly() {
    let backend = AcpAdapter::default_opencode();
    let (rc, _rx) = ctx();
    let cancel = rc.cancel.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();
    });

    let err = backend
        .run(
            task("Count slowly from 1 to 1000000.", Duration::from_secs(60)),
            rc,
        )
        .await
        .expect_err("cancel should produce an error");
    assert!(
        matches!(err, luft::core::contract::backend::BackendError::Cancelled),
        "expected Cancelled, got {err:?}"
    );
}
