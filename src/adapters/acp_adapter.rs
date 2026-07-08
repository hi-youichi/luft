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
use crate::core::contract::ids::{AgentId, RunId, TokenUsage};
use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use agent_client_protocol::schema::{
    ContentBlock, InitializeRequest, McpServer, McpServerStdio, NewSessionRequest, PromptRequest,
    ProtocolVersion, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionConfigKind, SessionConfigOptionCategory,
    SessionConfigSelectOptions, SessionNotification, SetSessionConfigOptionRequest, TextContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder};

/// Default idle timeout: if the ACP agent sends no protocol notification
/// for this duration, the session is killed.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// After a `structured_output` submission is captured, the agent has this long
/// to stop generating before the session is force-killed. The timer does NOT
/// reset on further activity — once the LLM has submitted its result, any
/// additional tool calls after that point are dropped and the session is
/// closed. This bounds the wait when the LLM continues producing tool calls
/// (e.g. `todo_write`) after submitting (see
/// `docs/issues/opengui-stories-2026-07-06-stuck-run.md` §3.3).
const POST_SUBMISSION_IDLE: Duration = Duration::from_secs(5);

/// What caused `idle_watchdog` to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogOutcome {
    /// LLM hung for `pre_idle` with no protocol activity and no submission.
    PreIdleTimeout,
    /// LLM submitted a valid `structured_output` but kept generating for
    /// `post_idle` after the submission. Caller should treat the captured
    /// submission as the result.
    PostSubmissionTimeout,
    /// All activity senders were dropped (channel closed). Treat as a clean
    /// shutdown.
    ChannelClosed,
}

/// ACP backend configuration.
#[derive(Debug, Clone)]
pub struct AcpConfig {
    /// Backend identifier (returned by `AgentBackend::id`).
    pub id: &'static str,
    /// Agent binary; resolved from `PATH`. Defaults to `opencode`.
    pub binary: PathBuf,
    /// Extra arguments passed to the agent binary (e.g. `["acp"]`).
    pub acp_args: Vec<String>,
    /// Optional `--log-level` passed to the agent.
    pub log_level: Option<String>,
    /// `initialize` handshake timeout.
    pub connect_timeout: Duration,
    /// Emit verbatim ACP `session/update` notifications as
    /// [`AgentEvent::AcpRaw`](crate::core::contract::event::AgentEvent::AcpRaw).
    /// On by default; the journal does not persist them (see `docs/design/acp-raw-events.md`).
    pub emit_raw_events: bool,
    /// Explicit allowlist of environment variable NAMES forwarded to the
    /// ACP subprocess. The parent process env is **not** inherited by default.
    ///
    /// Each name is looked up via `std::env::var` at spawn time; missing
    /// entries are silently skipped. Set to `vec![]` to forward **no**
    /// environment at all (subprocess starts with a fully empty env).
    ///
    /// AI provider credentials (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, ...)
    /// and the generic `MODEL` env var are deliberately excluded from the
    /// default — provider/model selection must come from the subprocess's
    /// own config files (`auth.json`, `config.json`) or explicit CLI
    /// arguments, not from the host shell. This is a security and
    /// reproducibility boundary: subprocess behavior does not silently
    /// change because the user happens to have a stray env var set.
    ///
    /// Default: [`AcpConfig::DEFAULT_ENV_PASSTHROUGH`] — the minimum set
    /// needed for the binary to bootstrap on the current OS (PATH, user
    /// dirs, temp, locale, shell).
    pub env_passthrough: Vec<String>,
    /// Model to use for LLM calls. Passed via ACP `session/set_config_option`
    /// with category `model`. If the agent does not support model selection,
    /// this is silently ignored.
    pub model: Option<String>,
}

impl AcpConfig {
    /// Environment variables forwarded by default. The minimum set needed
    /// for a subprocess to bootstrap on the current OS — **no** AI provider
    /// keys, no `MODEL`, no arbitrary shell state. Extend or override via
    /// [`AcpConfig::env_passthrough`].
    pub const DEFAULT_ENV_PASSTHROUGH: &'static [&'static str] = &[
        // OS / loader
        "PATH",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        // User / home
        "USERPROFILE",
        "HOME",
        "USER",
        "USERNAME",
        "LOGNAME",
        // Temp
        "TMPDIR",
        "TMP",
        "TEMP",
        // Locale
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        // Shell (sometimes needed for spawned sub-shells)
        "SHELL",
    ];
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            id: "opencode",
            binary: PathBuf::from("opencode"),
            acp_args: vec!["acp".to_string()],
            log_level: None,
            connect_timeout: Duration::from_secs(10),
            emit_raw_events: true,
            env_passthrough: Self::DEFAULT_ENV_PASSTHROUGH
                .iter()
                .map(|s| s.to_string())
                .collect(),
            model: None,
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
        self.config.id
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            mcp_injection: true,
            structured_output: true,
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
    cmd.args(&config.acp_args);
    if let Some(level) = &config.log_level {
        cmd.arg("--log-level").arg(level);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // SECURITY / REPRODUCIBILITY: do NOT inherit the parent process env.
    // The subprocess gets only the explicit allowlist in
    // `config.env_passthrough`. This prevents accidental leakage of
    // shell-level API keys (OPENAI_API_KEY, ANTHROPIC_API_KEY, ...) and
    // makes spawn behavior deterministic across host configurations.
    // Provider credentials and model selection must come from the
    // subprocess's own config files (`auth.json`, `config.json`) or
    // explicit arguments.
    cmd.env_clear();
    for name in &config.env_passthrough {
        if let Ok(value) = std::env::var(name) {
            cmd.env(name, value);
        }
    }

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

    // Activity channel: the notification handler sends a tick on every ACP
    // protocol message so the idle watchdog can distinguish a live (but slow)
    // agent from a hung one.
    let (activity_tx, mut activity_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    // Submission signal: fires exactly once when `structured_output` is
    // captured. The idle watchdog uses this to switch to a short, non-resetting
    // post-submission timeout so a chatty post-submission agent (e.g. one that
    // keeps calling `todo_write` after submitting) cannot hang the session.
    //
    // `Notify` (not `mpsc`) is used so `notified()` integrates cleanly with
    // `tokio::select!`: it resolves immediately if `notify_one()` was
    // already called (mpsc has a tight-loop-on-closed issue that is awkward
    // to work around here, and `try_recv` polling would add latency). See
    // `docs/issues/opengui-stories-2026-07-06-stuck-run.md` §3.3.
    let submit_signal = Arc::new(tokio::sync::Notify::new());

    // Clone for the watchdog (the original is moved into the conn_fut
    // closure; this clone is used by the outer `tokio::select!`).
    let submit_signal_watchdog = submit_signal.clone();

    // 2a. Prepare MCP server for structured output (if schema present).
    let _schema_guard = if let Some(ref schema) = task.output_schema {
        let schema_json = serde_json::to_string(schema)
            .map_err(|e| BackendError::Execution(format!("schema serialize: {e}")))?;
        let schema_file = tempfile::NamedTempFile::new()
            .map_err(|e| BackendError::Execution(format!("schema temp file: {e}")))?;
        std::fs::write(&schema_file, &schema_json)
            .map_err(|e| BackendError::Execution(format!("schema temp write: {e}")))?;
        let schema_path = schema_file.path().to_string_lossy().into_owned();
        tracing::debug!(schema_file = %schema_path, "prepared MCP structured-output server");
        Some(SchemaFileGuard(schema_file))
    } else {
        None
    };
    let schema_file_path = _schema_guard
        .as_ref()
        .map(|g| g.0.path().to_string_lossy().into_owned());

    // 3. Build + drive the ACP client connection.
    let conn_fut = {
        let acc_h = acc.clone();
        let acc_prompt = acc.clone();
        let events_h = events.clone();
        let stop_holder = stop_holder.clone();
        let activity_tx = activity_tx.clone();
        async move {
            Client
                .builder()
                .name("maestro")
                .on_receive_notification(
                    move |n: SessionNotification, _cx: ConnectionTo<Agent>| {
                        let acc_h = acc_h.clone();
                        let events_h = events_h.clone();
                        let activity_tx = activity_tx.clone();
                        let submit_signal = submit_signal.clone();
                        async move {
                            let _ = activity_tx.send(());
                            let kind = serde_json::to_value(&n.update)
                                .ok()
                                .and_then(|v| {
                                    v.get("sessionUpdate")
                                        .and_then(|v| v.as_str())
                                        .map(String::from)
                                })
                                .unwrap_or_else(|| "unknown".to_string());
                            tracing::debug!(%kind, "ACP session/update");
                            // Capture pre-update submission state so we can detect the
                            // None→Some transition on `structured_output` and signal the
                            // idle watchdog to switch to a short, non-resetting timer.
                            let was_submitted = acc_h
                                .structured_output
                                .lock()
                                .unwrap()
                                .is_some();
                            update_mapper::handle_update(
                                &n.update, run_id, agent_id, &acc_h, &events_h, emit_raw,
                            );
                            if !was_submitted
                                && acc_h.structured_output.lock().unwrap().is_some()
                            {
                                submit_signal.notify_one();
                                tracing::debug!(
                                    "ACP structured_output captured; watchdog switching to post-submission mode"
                                );
                            }
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
                            tracing::debug!(
                                approve,
                                options = req.options.len(),
                                "ACP permission request"
                            );
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
                .connect_with(transport, {
                    let model = config.model.clone();
                    move |conn: ConnectionTo<Agent>| {
                        let acc_prompt = acc_prompt.clone();
                        let model = model.clone();
                        async move {
                            tracing::debug!("ACP handshake: initialize");
                            conn.send_request(InitializeRequest::new(ProtocolVersion::V1))
                                .block_task()
                                .await?;
                            tracing::debug!("ACP handshake: session/new");
                            let ns = {
                                let req = NewSessionRequest::new(cwd);
                                let req = if let Some(ref sf) = schema_file_path {
                                    let maestro_bin = std::env::current_exe()
                                        .unwrap_or_else(|_| std::path::PathBuf::from("maestro"));
                                    let mcp = McpServerStdio::new(
                                        "maestro-structured-output",
                                        maestro_bin,
                                    )
                                    .args(vec![
                                        "mcp-structured-output".to_string(),
                                        "--schema-file".to_string(),
                                        sf.clone(),
                                    ]);
                                    req.mcp_servers(vec![McpServer::Stdio(mcp)])
                                } else {
                                    req
                                };
                                conn.send_request(req).block_task().await?
                            };
                            if let Some(ref model_name) = model {
                                if let Some(ref config_options) = ns.config_options {
                                    if let Some(model_option) = config_options.iter().find(|opt| {
                                        opt.category.as_ref() == Some(&SessionConfigOptionCategory::Model)
                                    }) {
                                        let valid = if let SessionConfigKind::Select(ref select) = model_option.kind {
                                            match &select.options {
                                                SessionConfigSelectOptions::Ungrouped(opts) => {
                                                    opts.iter().any(|o| o.value.0.as_ref() == model_name.as_str())
                                                }
                                                SessionConfigSelectOptions::Grouped(groups) => {
                                                    groups.iter().any(|g| {
                                                        g.options.iter().any(|o| o.value.0.as_ref() == model_name.as_str())
                                                    })
                                                }
                                                _ => false,
                                            }
                                        } else {
                                            false
                                        };
                                        if valid {
                                            tracing::debug!(model = %model_name, "ACP: setting session model");
                                            let req = SetSessionConfigOptionRequest::new(
                                                ns.session_id.clone(),
                                                model_option.id.clone(),
                                                model_name.clone(),
                                            );
                                            conn.send_request(req).block_task().await?;
                                        } else {
                                            tracing::warn!(
                                                model = %model_name,
                                                "ACP: requested model not available, using agent default"
                                            );
                                        }
                                    } else {
                                        tracing::debug!("ACP: agent does not support model selection");
                                    }
                                }
                            }
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
                            if let Some(ref u) = pr.usage {
                                tracing::debug!(
                                    input = u.input_tokens,
                                    output = u.output_tokens,
                                    total = u.total_tokens,
                                    "ACP prompt usage"
                                );
                                *acc_prompt.tokens.lock().unwrap() = TokenUsage {
                                    input: u.input_tokens,
                                    output: u.output_tokens,
                                    cache_read: u.cached_read_tokens.unwrap_or(0),
                                    cache_write: u.cached_write_tokens.unwrap_or(0),
                                };
                            }
                            Ok::<(), agent_client_protocol::Error>(())
                        }
                    }
                })
                .await
        }
    };

    // 4. Race the session against cancellation + idle timeout.
    //    The idle timeout resets on every ACP notification, so a long-running
    //    tool execution (with streaming updates) won't be killed — only a
    //    truly silent/hung agent will time out.
    let idle_timeout = task.timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT);
    let outcome = tokio::select! {
        r = conn_fut => r,
        _ = cancel.cancelled() => {
            tracing::debug!("ACP session cancelled");
            let _ = child.start_kill();
            return Err(BackendError::Cancelled);
        }
        res = idle_watchdog(
            idle_timeout,
            POST_SUBMISSION_IDLE,
            &mut activity_rx,
            submit_signal_watchdog,
        ) => {
            let _ = child.start_kill();
            match res {
                WatchdogOutcome::PreIdleTimeout => {
                    tracing::warn!(
                        idle_timeout_ms = idle_timeout.as_millis() as u64,
                        "ACP session idle timeout (no protocol activity)"
                    );
                    return Err(BackendError::Timeout);
                }
                WatchdogOutcome::ChannelClosed => {
                    tracing::debug!("ACP activity channel closed");
                    return Err(BackendError::Timeout);
                }
                WatchdogOutcome::PostSubmissionTimeout => {
                    // The LLM submitted a valid `structured_output` but kept
                    // generating tool calls after that (e.g. `todo_write`).
                    // We treat the captured submission as the session result:
                    // synthesize an `EndTurn` stop_reason and fall through to
                    // the normal collection path. The scheduler's
                    // schema-retry loop will then validate the payload
                    // against `task.output_schema`.
                    tracing::info!(
                        post_idle_ms = POST_SUBMISSION_IDLE.as_millis() as u64,
                        "ACP post-submission timeout; treating structured_output as result"
                    );
                    if stop_holder.lock().unwrap().is_none() {
                        *stop_holder.lock().unwrap() =
                            Some("EndTurn".to_string());
                    }
                    Ok(())
                }
            }
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
    let structured = acc.structured_output.lock().unwrap().take();
    Ok(result_collector::collect(
        &task, &stop, message, tokens, structured,
    ))
}

/// Completes after `idle` elapses with **no** signal on `rx`.
///
/// Each ACP `session/update` notification sends a `()` to `rx`, resetting
/// the idle timer. This lets a slow-but-alive agent (e.g. a long tool call
/// with periodic `ToolCallUpdate` events) run indefinitely while a truly
/// hung agent (no notifications at all) is killed after `idle`.
///
/// When `submit_signal.notify_one()` is called, the watchdog transitions
/// into a **post-submission** state: it switches to a short `post_idle`
/// timer that is **not** reset by further activity on `activity_rx`. This
/// bounds the wait once the LLM has submitted a valid `structured_output`
/// but keeps producing tool calls (e.g. `todo_write`) — the exact race
/// that produced the `story-UpdateDialog` and `story-MessageListSubmodule`
/// failures documented in
/// `docs/issues/opengui-stories-2026-07-06-stuck-run.md`.
///
/// `submit_signal` is a `tokio::sync::Notify` (not an mpsc) so the
/// `notified()` future integrates cleanly with `select!` and resolves
/// immediately if `notify_one()` was already called. `Notify` has no
/// "closed" state: if the handler is dropped without notifying, the
/// `notified()` future blocks indefinitely and the `pre_idle` timer is
/// the backstop.
async fn idle_watchdog(
    pre_idle: Duration,
    post_idle: Duration,
    activity_rx: &mut tokio::sync::mpsc::UnboundedReceiver<()>,
    submit_signal: Arc<tokio::sync::Notify>,
) -> WatchdogOutcome {
    let mut submitted = false;
    loop {
        if submitted {
            // Drain any trailing notifications: they DO NOT reset the timer.
            // The LLM has already submitted; we are giving it a fixed grace
            // period to stop, after which we kill the session.
            //
            // We deliberately do NOT wait on `submit_signal` or
            // `activity_rx` here: the submission is one-shot and any
            // further activity is irrelevant to whether the captured
            // `structured_output` is the result.
            while activity_rx.try_recv().is_ok() {}
            tokio::time::sleep(post_idle).await;
            return WatchdogOutcome::PostSubmissionTimeout;
        }
        tokio::select! {
            biased;
            _ = submit_signal.notified() => {
                submitted = true;
                tracing::debug!(
                    post_idle_ms = post_idle.as_millis() as u64,
                    "ACP watchdog entered post-submission mode"
                );
            }
            msg = activity_rx.recv() => match msg {
                Some(()) => { while activity_rx.try_recv().is_ok() {} }
                None => return WatchdogOutcome::ChannelClosed,
            },
            _ = tokio::time::sleep(pre_idle) => {
                return WatchdogOutcome::PreIdleTimeout;
            }
        }
    }
}

struct SchemaFileGuard(tempfile::NamedTempFile);

fn is_connection_closed(s: &str) -> bool {
    s.contains("receiver dropped")
        || s.contains("broken pipe")
        || s.contains("unexpected eof")
        || s.contains("connection closed")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── idle_watchdog ──────────────────────────────────────────────
    //
    // The watchdog has a two-state machine: pre-submission (timer resets
    // on activity) and post-submission (fixed short timer, no reset).
    // These tests exercise both paths and the transition between them.

    #[tokio::test]
    async fn idle_watchdog_fires_after_idle_period() {
        let (_atx, mut arx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let submit = Arc::new(tokio::sync::Notify::new());
        let r = tokio::time::timeout(
            Duration::from_millis(500),
            idle_watchdog(
                Duration::from_millis(50),
                Duration::from_millis(50),
                &mut arx,
                submit,
            ),
        )
        .await;
        let outcome = r.expect("should fire after idle period");
        assert_eq!(outcome, WatchdogOutcome::PreIdleTimeout);
    }

    #[tokio::test]
    async fn idle_watchdog_does_not_fire_with_activity() {
        let (atx, mut arx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let submit = Arc::new(tokio::sync::Notify::new());
        tokio::spawn(async move {
            for _ in 0..5 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let _ = atx.send(());
            }
        });
        let r = tokio::time::timeout(
            Duration::from_millis(80),
            idle_watchdog(
                Duration::from_millis(50),
                Duration::from_millis(50),
                &mut arx,
                submit,
            ),
        )
        .await;
        assert!(
            r.is_err(),
            "should not fire while activity is within idle window"
        );
    }

    #[tokio::test]
    async fn idle_watchdog_fires_after_activity_stops() {
        let (atx, mut arx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let submit = Arc::new(tokio::sync::Notify::new());
        let _ = atx.send(());
        drop(atx);
        let r = tokio::time::timeout(
            Duration::from_millis(30),
            idle_watchdog(
                Duration::from_millis(80),
                Duration::from_millis(80),
                &mut arx,
                submit,
            ),
        )
        .await;
        let outcome = r.expect("should return immediately when channel closes");
        assert_eq!(outcome, WatchdogOutcome::ChannelClosed);
    }

    // ── Post-submission mode (Fix M1) ───────────────────────────────
    //
    // The pre-submission watchdog keeps the session alive while the LLM
    // is actively emitting notifications. Once `submit_signal.notify_one()`
    // is called, the watchdog must switch to a short, non-resetting
    // post-idle timer so a chatty post-submission agent (one that keeps
    // calling `todo_write` after submitting `structured_output`) cannot
    // hang the session. This is the race that produced
    // `story-UpdateDialog` / `story-MessageListSubmodule` failures in
    // `docs/issues/opengui-stories-2026-07-06-stuck-run.md`.

    #[tokio::test]
    async fn idle_watchdog_enters_post_mode_after_submit_signal() {
        let (_atx, mut arx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let submit = Arc::new(tokio::sync::Notify::new());
        let submit_h = submit.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            submit_h.notify_one();
        });
        let r = tokio::time::timeout(
            Duration::from_millis(500),
            idle_watchdog(
                Duration::from_secs(60), // long pre-idle (should never fire)
                Duration::from_millis(50), // short post-idle
                &mut arx,
                submit,
            ),
        )
        .await;
        let outcome = r.expect("watchdog should return after post_idle");
        assert_eq!(outcome, WatchdogOutcome::PostSubmissionTimeout);
    }

    #[tokio::test]
    async fn idle_watchdog_post_mode_is_not_reset_by_activity() {
        // KEY INVARIANT: once the LLM has submitted, additional activity
        // ticks must NOT extend the wait. This is what prevents the
        // 2.5-minute hang observed in the opencode run.
        let (atx, mut arx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let submit = Arc::new(tokio::sync::Notify::new());
        submit.notify_one();
        tokio::spawn(async move {
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let _ = atx.send(());
            }
        });
        let start = std::time::Instant::now();
        let r = tokio::time::timeout(
            Duration::from_millis(500),
            idle_watchdog(
                Duration::from_secs(60),
                Duration::from_millis(80),
                &mut arx,
                submit,
            ),
        )
        .await;
        let outcome = r.expect("watchdog should return after post_idle");
        let elapsed = start.elapsed();
        assert_eq!(outcome, WatchdogOutcome::PostSubmissionTimeout);
        // The post-idle timer must dominate: even with 20 activity ticks
        // (totalling ~400 ms), the watchdog should fire at ~80 ms
        // post-submit. We allow generous slack for CI scheduling jitter.
        assert!(
            elapsed < Duration::from_millis(300),
            "post-mode timer was reset by activity: elapsed={elapsed:?}"
        );
    }

    #[tokio::test]
    async fn idle_watchdog_pre_mode_resets_on_activity() {
        // Sanity: pre-submission behavior is preserved (regression guard).
        let (atx, mut arx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let submit = Arc::new(tokio::sync::Notify::new());
        tokio::spawn(async move {
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(30)).await;
                let _ = atx.send(());
            }
        });
        let r = tokio::time::timeout(
            Duration::from_millis(200),
            idle_watchdog(
                Duration::from_millis(60),
                Duration::from_millis(60),
                &mut arx,
                submit,
            ),
        )
        .await;
        assert!(
            r.is_err(),
            "pre-mode should not fire while activity keeps resetting timer"
        );
    }
}
