//! `AcpAdapter` — drives an `opencode acp` subprocess as an ACP **client**.
//!
//! One [`AgentBackend::run`] call = one one-shot ACP session: spawn opencode,
//! `initialize` → `session/new` → `session/prompt`, stream `session/update`
//! notifications into Maestro progress events, then collect the result.
//!
//! ## Threading
//! The `agent-client-protocol` connection future is `!Send` (it drives a
//! `LocalSet`), but [`AgentBackend::run`] is `#[async_trait]` and therefore
//! `Send`. We bridge by running the whole session on a dedicated current-thread
//! runtime + `LocalSet` inside `spawn_blocking`, returning the `Send` result.

use super::{permission, result_collector, update_mapper};
use crate::core::contract::backend::{
    AgentBackend, AgentCapabilities, AgentResult, AgentTask, BackendError, RunContext,
};
use crate::core::contract::event::EventSender;
use crate::core::contract::ids::{AgentId, RunId};
use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use agent_client_protocol::schema::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, ProtocolVersion,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, TextContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(14400);

/// ACP backend configuration.
#[derive(Debug, Clone)]
pub struct AcpConfig {
    /// Agent binary; resolved from `PATH`. Defaults to `opencode`.
    pub binary: PathBuf,
    /// Optional `--log-level` passed to the agent.
    pub log_level: Option<String>,
    /// `initialize` handshake timeout.
    pub connect_timeout: Duration,
    /// Emit verbatim ACP `session/update` notifications as
    /// [`AgentEvent::AcpRaw`](crate::core::contract::event::AgentEvent::AcpRaw).
    /// On by default; the WS layer excludes these from the default subscription
    /// and the journal does not persist them (see `docs/design/acp-raw-events.md`).
    pub emit_raw_events: bool,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("opencode"),
            log_level: None,
            connect_timeout: Duration::from_secs(10),
            emit_raw_events: true,
        }
    }
}

/// ACP client backend for `opencode` (and compatible ACP agents).
pub struct AcpAdapter {
    config: AcpConfig,
}

impl AcpAdapter {
    pub fn new(config: AcpConfig) -> Self {
        Self { config }
    }

    /// Convenience constructor for the default `opencode acp` backend.
    pub fn default_opencode() -> Self {
        Self::new(AcpConfig::default())
    }
}

#[async_trait]
impl AgentBackend for AcpAdapter {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            mcp_injection: false, // MCP data-plane injection is P1
            structured_output: false,
            models: vec![],
        }
    }

    async fn run(&self, task: AgentTask, ctx: RunContext) -> Result<AgentResult, BackendError> {
        let config = self.config.clone();
        let cancel = ctx.cancel.clone();
        let events = ctx.events.clone();
        let run_id = ctx.run_id;

        // The ACP connection future is !Send → run it on its own current-thread
        // runtime + LocalSet, off the shared worker pool.
        let handle = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| BackendError::Execution(format!("acp runtime: {e}")))?;
            let local = tokio::task::LocalSet::new();
            local.block_on(&rt, run_acp_session(config, task, run_id, cancel, events))
        });

        handle
            .await
            .map_err(|e| BackendError::Execution(format!("acp task join: {e}")))?
    }
}

/// The `!Send` one-shot session, driven inside a `LocalSet`.
///
/// The `backend` span carries `run_id`/`agent_id` so the session's diagnostics
/// inherit them (see `docs/design/program-logging.md`).
#[tracing::instrument(
    name = "backend",
    skip_all,
    fields(run_id = %run_id, agent_id = %task.agent_id, backend = "opencode")
)]
async fn run_acp_session(
    config: AcpConfig,
    task: AgentTask,
    run_id: RunId,
    cancel: tokio_util::sync::CancellationToken,
    events: EventSender,
) -> Result<AgentResult, BackendError> {
    // 1. Spawn `opencode acp`.
    let mut cmd = tokio::process::Command::new(&config.binary);
    cmd.arg("acp");
    if let Some(level) = &config.log_level {
        cmd.arg("--log-level").arg(level);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().map_err(|e| {
        tracing::error!(binary = %config.binary.display(), error = %e, "failed to spawn ACP backend");
        BackendError::Spawn(format!("failed to spawn {}: {e}", config.binary.display()))
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| BackendError::Spawn("no child stdin".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| BackendError::Spawn("no child stdout".into()))?;
    let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());

    // 2. Shared state for handlers + result collection.
    let acc = Arc::new(update_mapper::Accumulator::new());
    let stop_holder: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let agent_id: AgentId = task.agent_id;
    let cwd = std::fs::canonicalize(&task.workdir).unwrap_or_else(|_| task.workdir.clone());
    let prompt = task.prompt.clone();
    let policy = task.allowlist.clone();
    let emit_raw = config.emit_raw_events;

    // 3. Build + drive the ACP client connection.
    let conn_fut = {
        let acc_h = acc.clone();
        let events_h = events.clone();
        let stop_holder = stop_holder.clone();
        async move {
            Client
                .builder()
                .name("maestro")
                .on_receive_notification(
                    move |n: SessionNotification, _cx: ConnectionTo<Agent>| {
                        let acc_h = acc_h.clone();
                        let events_h = events_h.clone();
                        async move {
                            tracing::trace!("ACP session/update received");
                            update_mapper::handle_update(
                                &n.update, run_id, agent_id, &acc_h, &events_h, emit_raw,
                            );
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    move |req: RequestPermissionRequest,
                          responder: Responder<RequestPermissionResponse>,
                          _conn: ConnectionTo<Agent>| {
                        let policy = policy.clone();
                        async move {
                            let inputs = permission::extract_inputs(&req);
                            let approve = matches!(
                                permission::decide(policy.as_ref(), &inputs),
                                permission::Decision::Approve
                            );
                            tracing::debug!(approve, options = req.options.len(), "ACP permission request");
                            let outcome = match (approve, req.options.first()) {
                                (true, Some(opt)) => RequestPermissionOutcome::Selected(
                                    SelectedPermissionOutcome::new(opt.option_id.clone()),
                                ),
                                _ => RequestPermissionOutcome::Cancelled,
                            };
                            responder.respond(RequestPermissionResponse::new(outcome))
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, move |conn: ConnectionTo<Agent>| async move {
                    tracing::debug!("ACP handshake: initialize");
                    conn.send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await?;
                    tracing::debug!("ACP handshake: session/new");
                    let ns = conn
                        .send_request(NewSessionRequest::new(cwd))
                        .block_task()
                        .await?;
                    tracing::debug!("ACP handshake: session/prompt");
                    let pr = conn
                        .send_request(PromptRequest::new(
                            ns.session_id,
                            vec![ContentBlock::Text(TextContent::new(prompt))],
                        ))
                        .block_task()
                        .await?;
                    tracing::debug!(stop_reason = ?pr.stop_reason, "ACP prompt complete");
                    *stop_holder.lock().unwrap() = Some(format!("{:?}", pr.stop_reason));
                    Ok::<(), agent_client_protocol::Error>(())
                })
                .await
        }
    };

    // 4. Race the session against cancellation + wall-clock timeout.
    let timeout = task.timeout.unwrap_or(DEFAULT_TIMEOUT);
    let outcome = tokio::select! {
        r = conn_fut => r,
        _ = cancel.cancelled() => {
            tracing::debug!("ACP session cancelled");
            let _ = child.start_kill();
            return Err(BackendError::Cancelled);
        }
        _ = tokio::time::sleep(timeout) => {
            tracing::warn!(timeout_ms = timeout.as_millis() as u64, "ACP session timed out");
            let _ = child.start_kill();
            return Err(BackendError::Timeout);
        }
    };
    let _ = child.start_kill();

    outcome.map_err(|e| {
        let s = e.to_string();
        if is_connection_closed(&s) {
            tracing::warn!("ACP connection closed");
            BackendError::Protocol("connection closed".into())
        } else {
            tracing::error!(error = %s, "ACP protocol error");
            BackendError::Protocol(s)
        }
    })?;

    // 5. Collect the result from accumulated state.
    let stop = stop_holder.lock().unwrap().take().unwrap_or_default();
    let message = std::mem::take(&mut *acc.message.lock().unwrap());
    let tokens = *acc.tokens.lock().unwrap();
    Ok(result_collector::collect(&task, &stop, message, tokens))
}

fn is_connection_closed(s: &str) -> bool {
    s.contains("receiver dropped")
        || s.contains("broken pipe")
        || s.contains("unexpected eof")
        || s.contains("connection closed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_is_opencode() {
        let c = AcpConfig::default();
        assert_eq!(c.binary, PathBuf::from("opencode"));
        assert_eq!(c.connect_timeout, Duration::from_secs(10));
    }

    #[test]
    fn id_is_opencode() {
        assert_eq!(AcpAdapter::default_opencode().id(), "opencode");
    }
}
