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
    SelectedPermissionOutcome, SessionNotification, TextContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder};

/// Default idle timeout: if the ACP agent sends no protocol notification
/// for this duration, the session is killed.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

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
                    move |conn: ConnectionTo<Agent>| {
                        let acc_prompt = acc_prompt.clone();
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
        _ = idle_watchdog(idle_timeout, &mut activity_rx) => {
            tracing::warn!(
                idle_timeout_ms = idle_timeout.as_millis() as u64,
                "ACP session idle timeout (no protocol activity)"
            );
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
async fn idle_watchdog(idle: Duration, rx: &mut tokio::sync::mpsc::UnboundedReceiver<()>) {
    loop {
        tokio::select! {
            biased;
            msg = rx.recv() => match msg {
                Some(()) => {
                    while rx.try_recv().is_ok() {}
                }
                None => return,
            },
            _ = tokio::time::sleep(idle) => return,
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

    // ------------------------------------------------------------------
    // AcpConfig
    // ------------------------------------------------------------------

    #[test]
    fn config_default_is_opencode() {
        let c = AcpConfig::default();
        assert_eq!(c.binary, PathBuf::from("opencode"));
        assert_eq!(c.connect_timeout, Duration::from_secs(10));
        assert!(c.emit_raw_events);
        assert!(c.log_level.is_none());
    }

    #[test]
    fn config_clone_and_debug() {
        let c = AcpConfig::default();
        let _cloned = c.clone();
        let _debug = format!("{c:?}");
    }

    // ------------------------------------------------------------------
    // AcpAdapter
    // ------------------------------------------------------------------

    #[test]
    fn id_is_opencode() {
        assert_eq!(AcpAdapter::default_opencode().id(), "opencode");
    }

    #[test]
    fn capabilities_are_correct() {
        let adapter = AcpAdapter::default_opencode();
        let caps = adapter.capabilities();
        assert!(caps.streaming);
        assert!(caps.mcp_injection);
        assert!(caps.structured_output);
        assert!(caps.models.is_empty());
    }

    #[test]
    fn new_adapter_accepts_custom_config() {
        let config = AcpConfig {
            id: "custom-agent",
            ..Default::default()
        };
        let adapter = AcpAdapter::new(config);
        assert_eq!(adapter.id(), "custom-agent");
        assert!(adapter.capabilities().streaming);
    }

    #[test]
    fn default_env_passthrough_excludes_ai_provider_vars() {
        // Security boundary: the default passthrough must not include any
        // common AI provider credential / model-selection variables, so
        // a stray shell `OPENAI_API_KEY` or `MODEL` cannot leak into
        // the subprocess and override the subprocess's own config.
        let c = AcpConfig::default();
        for forbidden in [
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_MODEL",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_MODEL",
            "MODEL",
            "LOOM_AUX_MODEL",
        ] {
            assert!(
                !c.env_passthrough.iter().any(|v| v == forbidden),
                "default env_passthrough must not include {forbidden} (would defeat env isolation)"
            );
        }
        // Sanity: OS bootstrap vars that the subprocess needs to start
        // (PATH at minimum) must be present.
        for required in ["PATH"] {
            assert!(
                c.env_passthrough.iter().any(|v| v == required),
                "default env_passthrough must include {required}"
            );
        }
        // Sanity: the default allowlist is a closed list, not "inherit
        // everything" (the latter would be a security hole).
        assert!(
            c.env_passthrough.len() <= AcpConfig::DEFAULT_ENV_PASSTHROUGH.len(),
            "default env_passthrough size {} exceeds DEFAULT_ENV_PASSTHROUGH size {}",
            c.env_passthrough.len(),
            AcpConfig::DEFAULT_ENV_PASSTHROUGH.len()
        );
    }

    #[test]
    fn empty_env_passthrough_is_supported() {
        // Users who want a fully empty env (no PATH, no HOME) can opt
        // in explicitly. This is the "hardest" isolation mode.
        let config = AcpConfig {
            env_passthrough: vec![],
            ..Default::default()
        };
        let _adapter = AcpAdapter::new(config);
    }

    #[test]
    fn default_opencode_creates_adapter() {
        let adapter = AcpAdapter::default_opencode();
        assert_eq!(adapter.id(), "opencode");
        assert!(adapter.capabilities().streaming);
    }

    // ------------------------------------------------------------------
    // is_connection_closed
    // ------------------------------------------------------------------

    #[test]
    fn is_connection_closed_true_for_receiver_dropped() {
        assert!(is_connection_closed("receiver dropped"));
        assert!(is_connection_closed("error: receiver dropped"));
    }

    #[test]
    fn is_connection_closed_true_for_broken_pipe() {
        assert!(is_connection_closed("broken pipe"));
        assert!(is_connection_closed("broken pipe: write error"));
    }

    #[test]
    fn is_connection_closed_true_for_unexpected_eof() {
        assert!(is_connection_closed("unexpected eof"));
        assert!(is_connection_closed("io error: unexpected eof"));
    }

    #[test]
    fn is_connection_closed_true_for_connection_closed_phrase() {
        assert!(is_connection_closed("connection closed"));
        assert!(is_connection_closed("error: connection closed"));
    }

    #[test]
    fn is_connection_closed_false_for_other_strings() {
        assert!(!is_connection_closed(""));
        assert!(!is_connection_closed("some random error"));
        assert!(!is_connection_closed("receiver"));
        assert!(!is_connection_closed("pipe"));
        assert!(!is_connection_closed("unexpected"));
        assert!(!is_connection_closed("closed"));
        assert!(!is_connection_closed("timed out"));
        assert!(!is_connection_closed("protocol error"));
    }

    // ------------------------------------------------------------------
    // Helpers for AgentBackend::run tests
    // ------------------------------------------------------------------

    fn test_task(timeout_secs: u64, output_schema: Option<serde_json::Value>) -> AgentTask {
        AgentTask {
            agent_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            prompt: "hello".into(),
            model: None,
            allowlist: None,
            workdir: PathBuf::from("/tmp"),
            mcp_endpoint: None,
            timeout: Some(Duration::from_secs(timeout_secs)),
            output_schema,
            description: None,
            role: None,
            name: None,
            agent_seq: 0,
        }
    }

    fn test_context(cancel: Option<tokio_util::sync::CancellationToken>) -> RunContext {
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let cancel = cancel.unwrap_or_else(tokio_util::sync::CancellationToken::new);
        RunContext {
            run_id: uuid::Uuid::now_v7(),
            cancel,
            events: tx,
        }
    }

    /// Create a temporary shell script that sleeps for `secs` and ignores all
    /// arguments (the ACP adapter always passes `acp` as the first argument).
    /// The script lives inside the returned `TempDir` so it is automatically
    /// cleaned up when the directory is dropped.
    #[cfg(unix)]
    fn blocking_script(secs: u64) -> (std::path::PathBuf, tempfile::TempDir) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sleep_script.sh");
        let mut f = std::fs::File::create(&path).expect("create script");
        writeln!(f, "#!/bin/sh").expect("write shebang");
        writeln!(f, "sleep {secs}").expect("write sleep");
        drop(f);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod +x");
        (path, dir)
    }

    // ------------------------------------------------------------------
    // AgentBackend::run  –  integration-level tests
    // ------------------------------------------------------------------

    // ── Spawn error (binary not found) ──────────────────────────────

    #[tokio::test]
    async fn run_with_nonexistent_binary_returns_spawn_error() {
        let config = AcpConfig {
            binary: PathBuf::from("/nonexistent-binary-for-testing"),
            log_level: Some("debug".into()),
            ..Default::default()
        };
        let adapter = AcpAdapter::new(config);
        let task = test_task(5, None);
        let ctx = test_context(None);
        let result = adapter.run(task, ctx).await;
        assert!(
            matches!(&result, Err(BackendError::Spawn(_))),
            "expected Spawn error, got: {result:?}"
        );
    }

    // ── Non-ACP binary that produces output → error (Protocol or Timeout) ──
    //
    // Using a no-op binary (echo/true) as a non-ACP binary. The ACP client
    // either fails to parse the output (Protocol) or hits the connection-closed
    // path (Timeout), depending on timing. Both are valid error outcomes.

    fn echo_binary() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()))
        } else {
            PathBuf::from("/bin/echo")
        }
    }

    fn true_binary() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()))
        } else {
            PathBuf::from("/usr/bin/true")
        }
    }

    #[tokio::test]
    async fn run_with_non_acp_binary_returns_error() {
        let config = AcpConfig {
            binary: echo_binary(),
            ..Default::default()
        };
        let adapter = AcpAdapter::new(config);
        let task = test_task(5, None);
        let ctx = test_context(None);
        let result = adapter.run(task, ctx).await;
        assert!(
            matches!(
                &result,
                Err(BackendError::Protocol(_)) | Err(BackendError::Timeout)
            ),
            "expected Protocol or Timeout error, got: {result:?}"
        );
    }

    // ── Pre-cancelled token → Cancelled ────────────────────────────

    #[tokio::test]
    async fn run_with_precancelled_token_returns_cancelled() {
        let config = AcpConfig {
            binary: true_binary(),
            ..Default::default()
        };
        let adapter = AcpAdapter::new(config);
        let task = test_task(60, None);
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let ctx = test_context(Some(cancel));
        let result = adapter.run(task, ctx).await;
        assert!(
            matches!(&result, Err(BackendError::Cancelled)),
            "expected Cancelled, got: {result:?}"
        );
    }

    // ── Cancellation during the session ────────────────────────────

    #[cfg(unix)]
    #[tokio::test]
    async fn run_cancellation_during_session() {
        let (script_path, _dir) = blocking_script(120);
        let config = AcpConfig {
            binary: script_path,
            ..Default::default()
        };
        let adapter = AcpAdapter::new(config);
        let task = test_task(120, None);
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = test_context(Some(cancel.clone()));

        let handle = tokio::spawn(async move { adapter.run(task, ctx).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(15), handle)
            .await
            .expect("test timed out waiting for cancellation")
            .expect("join error");
        assert!(
            matches!(&result, Err(BackendError::Cancelled)),
            "expected Cancelled during session, got: {result:?}"
        );
    }

    // ── Timeout ────────────────────────────────────────────────────

    #[cfg(unix)]
    #[tokio::test]
    async fn run_with_timeout() {
        let (script_path, _dir) = blocking_script(120);
        let config = AcpConfig {
            binary: script_path,
            ..Default::default()
        };
        let adapter = AcpAdapter::new(config);
        let task = test_task(120, None);
        // Override with an extremely short timeout.
        let task = AgentTask {
            timeout: Some(Duration::from_millis(100)),
            ..task
        };
        let ctx = test_context(None);

        let handle = tokio::spawn(async move { adapter.run(task, ctx).await });

        let result = tokio::time::timeout(Duration::from_secs(15), handle)
            .await
            .expect("test timed out")
            .expect("join error");
        assert!(
            matches!(&result, Err(BackendError::Timeout)),
            "expected Timeout, got: {result:?}"
        );
    }

    // ── Output-schema guard ────────────────────────────────────────
    //
    // Using a non-ACP binary with an output schema. The result is
    // either Protocol or Timeout depending on timing (see note above).

    #[tokio::test]
    async fn run_with_output_schema_creates_guard() {
        let config = AcpConfig {
            binary: echo_binary(),
            ..Default::default()
        };
        let adapter = AcpAdapter::new(config);
        let task = test_task(5, Some(serde_json::json!({"type": "object"})));
        let ctx = test_context(None);
        let result = adapter.run(task, ctx).await;
        assert!(
            matches!(
                &result,
                Err(BackendError::Protocol(_)) | Err(BackendError::Timeout)
            ),
            "expected Protocol or Timeout with schema, got: {result:?}"
        );
    }

    // ── idle_watchdog ──────────────────────────────────────────────

    #[tokio::test]
    async fn idle_watchdog_fires_after_idle_period() {
        let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let r = tokio::time::timeout(
            Duration::from_millis(500),
            idle_watchdog(Duration::from_millis(50), &mut rx),
        )
        .await;
        assert!(r.is_ok(), "should fire after idle period");
    }

    #[tokio::test]
    async fn idle_watchdog_does_not_fire_with_activity() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();

        tokio::spawn(async move {
            for _ in 0..5 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let _ = tx.send(());
            }
        });

        let r = tokio::time::timeout(
            Duration::from_millis(80),
            idle_watchdog(Duration::from_millis(50), &mut rx),
        )
        .await;
        assert!(
            r.is_err(),
            "should not fire while activity is within idle window"
        );
    }

    #[tokio::test]
    async fn idle_watchdog_fires_after_activity_stops() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let _ = tx.send(());
        drop(tx);

        let r = tokio::time::timeout(
            Duration::from_millis(30),
            idle_watchdog(Duration::from_millis(80), &mut rx),
        )
        .await;
        assert!(r.is_ok(), "should return immediately when channel closes");
    }
}
