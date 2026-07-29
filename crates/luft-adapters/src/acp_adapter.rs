//! `AcpAdapter` — drives an `opencode acp` subprocess as an ACP **client**.
//!
//! One [`AgentBackend::run`] call = one one-shot ACP session: spawn opencode,
//! `initialize` → `session/new` → `session/prompt`, stream `session/update`
//! notifications into Luft progress events, then collect the result.
//!
//! ## Threading
//! The `agent-client-protocol` connection future is `!Send` (it drives a
//! `LocalSet`), but [`AgentBackend::run`] is `#[async_trait]` and therefore
//! `Send`. We bridge by running the whole session on a dedicated current-thread
//! runtime + `LocalSet` inside `spawn_blocking`, returning the `Send` result.
//!
//! ## Structure
//! [`run_acp_session`] is the orchestrator and delegates each phase to a
//! dedicated helper:
//!  1. [`spawn_agent`] — fork `opencode acp` and wire up stdin/stdout.
//!  2. (inline) — assemble shared `Arc`s and channels.
//!  3. [`prepare_schema_mcp`] — write a temp JSON Schema file when an
//!     `output_schema` is present.
//!     Alongside this, [`write_workflow_skill_files`] writes the Luft
//!     workflow-authoring skill (`luft_skills::WORKFLOW_SKILL`) into
//!     whichever skill-directory convention matches the backend actually
//!     being spawned (see `skill_dirs_for_backend`) — see
//!     `docs/design/skill-implementation.md` §4.
//!  4. [`drive_connection`] — build the `Client` builder and wire up
//!     notification/permission handlers.
//!  5. [`run_handshake_and_prompt`] — `initialize` → `session/new` (with
//!     optional `set_config_option`) → `session/prompt`.
//!  6. [`collect_session_result`] — assemble the final [`AgentResult`].

use super::{permission, result_collector, update_mapper};
use async_trait::async_trait;
use luft_core::contract::backend::{
    AgentBackend, AgentCapabilities, AgentResult, AgentTask, BackendError, RunContext, ToolPolicy,
};
use luft_core::contract::event::EventSender;
#[cfg(feature = "unstable_end_turn_token_usage")]
use luft_core::contract::ids::TokenUsage;
use luft_core::contract::ids::{AgentId, RunId};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// The byte-stream transport used between Luft and the ACP subprocess.
type AcpTransport =
    ByteStreams<Compat<tokio::process::ChildStdin>, Compat<tokio::process::ChildStdout>>;

use agent_client_protocol::schema::{
    ContentBlock, Implementation, InitializeRequest, McpServer, McpServerStdio, NewSessionRequest,
    NewSessionResponse, PromptRequest, ProtocolVersion, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionConfigKind, SessionConfigOptionCategory, SessionConfigSelectOptions, SessionId,
    SessionNotification, SetSessionConfigOptionRequest, StopReason, TextContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder};

/// Default idle timeout: if the ACP agent sends no protocol notification
/// for this duration, the session is killed.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// After a `workflow_validate_schema` submission is captured, the agent has this long
/// to stop generating before the session is force-killed. The timer does NOT
/// reset on further activity — once the LLM has submitted its result, any
/// additional tool calls after that point are dropped and the session is
/// closed. This bounds the wait when the LLM continues producing tool calls
/// (e.g. `todo_write`) after submitting (see
/// `docs/issues/opengui-stories-2026-07-06-stuck-run.md` §3.3).
const POST_SUBMISSION_IDLE: Duration = Duration::from_secs(5);

/// Stable PascalCase spelling of `StopReason::EndTurn` — used when synthesizing
/// a stop reason after a post-submission timeout so the stored value matches
/// what [`stop_reason_as_str`] would have produced for a real `EndTurn`
/// response. This is the **single source of truth** for the synthesized
/// spelling; do not inline `"EndTurn"` elsewhere.
const STOP_REASON_END_TURN: &str = "EndTurn";

/// What caused `idle_watchdog` to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogOutcome {
    /// LLM hung for `pre_idle` with no protocol activity and no submission.
    PreIdleTimeout,
    /// LLM submitted a valid `workflow_validate_schema` but kept generating for
    /// `post_idle` after the submission. Caller should treat the captured
    /// submission as the result.
    PostSubmissionTimeout,
    /// All activity senders were dropped (channel closed). Treat as a clean
    /// shutdown.
    ChannelClosed,
}

/// Bundled state used by the ACP connection handlers. Holding these as a
/// single struct keeps the closure call sites thin and easy to scan.
struct SessionState {
    acc: Arc<update_mapper::Accumulator>,
    stop_holder: Arc<Mutex<Option<String>>>,
    events: EventSender,
    activity_tx: tokio::sync::mpsc::UnboundedSender<()>,
    activity_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    submit_signal: Arc<tokio::sync::Notify>,
    run_id: RunId,
    agent_id: AgentId,
    emit_raw: bool,
    policy: Option<ToolPolicy>,
    prompt: String,
    cwd: PathBuf,
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
    /// [`AgentEvent::AcpRaw`](luft_core::contract::event::AgentEvent::AcpRaw).
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
    /// Fixed, non-sensitive variables for the ACP subprocess. These override
    /// inherited values of the same name.
    pub env: BTreeMap<String, String>,
    /// Model to use for LLM calls. Passed via ACP `session/set_config_option`
    /// with category `model`. If the agent does not support model selection,
    /// this is silently ignored.
    pub model: Option<String>,
    /// Override for the binary path used to spawn the
    /// `luft mcp-structured-output` subprocess (see `session_new`).
    ///
    /// Defaults to `None`, which falls back to `std::env::current_exe()` —
    /// correct when this code is actually running as the `luft` CLI binary.
    /// That assumption breaks in two known cases: integration tests (where
    /// `current_exe()` resolves to the test harness binary, not `luft.exe`),
    /// and any host program that embeds `luft`/`luft-adapters` as a library
    /// rather than running as the `luft` CLI (`current_exe()` would resolve
    /// to the host's own binary). Set this explicitly in either situation —
    /// leaving it unset makes the structured-output MCP server silently
    /// unreachable, which surfaces as plain-text agent output instead of a
    /// schema-validated result.
    pub luft_binary: Option<PathBuf>,
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
        // Windows: app data / program files (needed by npx, Node, etc.)
        "APPDATA",
        "LOCALAPPDATA",
        "ProgramFiles",
        "ProgramFiles(x86)",
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
            env: BTreeMap::new(),
            model: None,
            luft_binary: None,
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

    /// Returns the configuration this adapter was built with. Useful for
    /// introspection / diagnostics and for tests that need to distinguish
    /// adapter instances beyond their [`AgentBackend::id`].
    pub fn config(&self) -> &AcpConfig {
        &self.config
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
            workflow_validate_schema: true,
            models: vec![],
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
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

/// Orchestrator. Each numbered phase is delegated to a named helper below.
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
    // 1. Spawn the agent subprocess and build the byte-stream transport.
    let (mut child, transport) = spawn_agent(&config)?;

    // 2. Shared state: accumulator, stop-reason slot, activity channel, and
    //    the submission `Notify` that flips the idle watchdog into
    //    post-submission mode. The `submit_signal` is `Arc`d so both the
    //    watchdog (kept here) and the notification handler (inside the
    //    connection builder) can hold a handle.
    let (activity_tx, activity_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let submit_signal = Arc::new(tokio::sync::Notify::new());
    let mut state = SessionState {
        acc: Arc::new(update_mapper::Accumulator::new()),
        stop_holder: Arc::new(Mutex::new(None)),
        events: events.clone(),
        activity_tx,
        activity_rx,
        submit_signal: submit_signal.clone(),
        run_id,
        agent_id: task.agent_id,
        emit_raw: config.emit_raw_events,
        policy: task.allowlist.clone(),
        prompt: task.prompt.clone(),
        cwd: std::fs::canonicalize(&task.workdir).unwrap_or_else(|_| task.workdir.clone()),
    };

    // Write the workflow-authoring skill into the spawned backend's own
    // skill-directory convention, if it has one. Best-effort, non-fatal —
    // see `write_workflow_skill_files`.
    write_workflow_skill_files(config.id, &state.cwd);

    // 3. Optional structured-output MCP server: serialise the JSON Schema to
    //    a temp file so the `luft mcp-structured-output` subprocess can
    //    validate the agent's final payload.
    let schema_guard = prepare_schema_mcp(task.output_schema.as_ref())?;
    let schema_file_path = schema_guard
        .as_ref()
        .map(|g| g.0.path().to_string_lossy().into_owned());

    // 4. Build the connection future, then race it against cancel + idle
    //    timeout. The watchdog's `submit_signal` is a clone of the one
    //    inside `state` so `notify_one()` from the notification handler
    //    reaches both.
    let conn_fut = drive_connection(
        &state,
        transport,
        schema_file_path,
        // Per-task model overrides the backend's configured default model,
        // so `agent({backend = "codex", model = "o4-mini"})` actually uses
        // o4-mini for that call. Falls back to the backend's `config.model`.
        task.model.clone().or(config.model.clone()),
        config.luft_binary.clone(),
        config.id,
    );
    let idle_timeout = task.timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT);

    // 5. Race the session against cancellation + idle timeout.
    //    The idle timeout resets on every ACP notification, so a long-running
    //    tool execution (with streaming updates) won't be killed — only a
    //    truly silent/hung agent will time out.
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
            &mut state.activity_rx,
            state.submit_signal.clone(),
        ) => {
            handle_watchdog_outcome(res, &mut child, &state.stop_holder, idle_timeout)?;
            Ok(())
        }
    };
    let _ = child.start_kill();

    outcome.map_err(classify_protocol_error)?;

    // 6. Collect the final result from the accumulator + stop-reason slot.
    Ok(collect_session_result(&task, &state))
}

// ─── Phase 1: spawn ────────────────────────────────────────────────────────

/// Build the `Command` for `opencode acp`, spawn it, and wrap its stdio in a
/// `ByteStreams` transport suitable for the ACP client.
///
/// SECURITY / REPRODUCIBILITY: the parent process env is **never** inherited.
/// Only the explicit allowlist in [`AcpConfig::env_passthrough`] is forwarded.
/// This prevents accidental leakage of shell-level API keys and makes spawn
/// behavior deterministic across host configurations.
fn spawn_agent(config: &AcpConfig) -> Result<(tokio::process::Child, AcpTransport), BackendError> {
    let mut cmd = tokio::process::Command::new(&config.binary);
    cmd.args(&config.acp_args);
    if let Some(level) = &config.log_level {
        cmd.arg("--log-level").arg(level);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    cmd.env_clear();
    for name in &config.env_passthrough {
        if let Ok(value) = std::env::var(name) {
            cmd.env(name, value);
        }
    }
    cmd.envs(&config.env);

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
    Ok((child, transport))
}

// ─── Phase 3: schema MCP ────────────────────────────────────────────────────

/// If a JSON Schema was supplied, serialise it to a temp file and return a
/// guard that deletes the file when dropped. The file path is later injected
/// into the `session/new` request as the `--schema-file` arg of a
/// `luft mcp-structured-output` subprocess.
fn prepare_schema_mcp(
    schema: Option<&serde_json::Value>,
) -> Result<Option<SchemaFileGuard>, BackendError> {
    let Some(schema) = schema else {
        return Ok(None);
    };
    let schema_json = serde_json::to_string(schema)
        .map_err(|e| BackendError::Execution(format!("schema serialize: {e}")))?;
    let schema_file = tempfile::NamedTempFile::new()
        .map_err(|e| BackendError::Execution(format!("schema temp file: {e}")))?;
    std::fs::write(&schema_file, &schema_json)
        .map_err(|e| BackendError::Execution(format!("schema temp write: {e}")))?;
    let path = schema_file.path().to_string_lossy().into_owned();
    tracing::debug!(schema_file = %path, "prepared MCP structured-output server");
    Ok(Some(SchemaFileGuard(schema_file)))
}

struct SchemaFileGuard(tempfile::NamedTempFile);

// ─── Phase 3b: workflow skill files ─────────────────────────────────────────

/// Maps a backend id to its skill-directory convention, relative to the
/// spawned process's working directory.
///
/// Checked against each backend's actual source (not assumed):
/// - `codex` / `opencode` — both resolve to `.agents/skills`: codex via
///   `external_agent_config.rs::home_target_skills_dir()`, opencode via
///   `skill/index.ts`'s `AGENTS_EXTERNAL_DIR` (opencode additionally reads
///   `.claude/skills`, which is covered by the `codex`/`opencode` case
///   sharing `.agents/skills` — no separate branch needed for that overlap).
/// - `claude` / `claude-code` — not a spawnable backend yet (no ACP bridge
///   wired up), kept here so the mapping is already correct the day one is
///   added, without another edit to this function.
///
/// Returns `&[]` for any other id — an unrecognized backend just gets no
/// skill written, which is the same as not having this feature at all.
/// Programs that embed `luft` as a library and drive their own
/// `AgentBackend` never reach this code path at all; they consume the skill
/// through `luft_skills::WORKFLOW_SKILL` directly (the library channel, see
/// `docs/design/skill-architecture.md` §4.1).
fn skill_dirs_for_backend(backend_id: &str) -> &'static [&'static str] {
    match backend_id {
        "codex" | "opencode" => &[".agents/skills"],
        "claude" | "claude-code" => &[".claude/skills"],
        _ => &[],
    }
}

/// Writes the Luft workflow-authoring skill into the skill-directory
/// convention for `backend_id`, relative to `working_folder`.
///
/// Every run gets a fresh copy — this is not a persistent install with a
/// manifest tracking user edits (unlike loom's own `sync.rs` for its
/// built-in skills). There's nothing to preserve: `working_folder` is
/// this run's own scratch space, and the same content gets rewritten on
/// every subsequent run through the same directory. Best-effort: a write
/// failure is logged and does not fail the run — skill delivery is a
/// quality-of-life aid for the spawned agent, not a correctness requirement.
fn write_workflow_skill_files(backend_id: &str, working_folder: &Path) {
    for base in skill_dirs_for_backend(backend_id) {
        let skill_dir = working_folder.join(base).join("workflow");
        if let Err(e) = luft_skills::write_to_dir(&skill_dir, &luft_skills::WORKFLOW_SKILL) {
            tracing::warn!(
                dir = %skill_dir.display(),
                error = %e,
                "failed to write workflow skill"
            );
        }
    }
}

// ─── Phase 4: drive the connection ──────────────────────────────────────────

/// Build the `Client` connection, attaching notification and permission
/// handlers and the handshake+prompt driver as the `main_fn`.
fn drive_connection(
    state: &SessionState,
    transport: AcpTransport,
    schema_file_path: Option<String>,
    model: Option<String>,
    luft_binary: Option<PathBuf>,
    backend_id: &'static str,
) -> impl std::future::Future<Output = Result<(), agent_client_protocol::Error>> {
    let acc = state.acc.clone();
    let events = state.events.clone();
    let stop_holder = state.stop_holder.clone();
    let activity_tx = state.activity_tx.clone();
    let submit_signal = state.submit_signal.clone();
    let run_id = state.run_id;
    let agent_id = state.agent_id;
    let emit_raw = state.emit_raw;
    let policy = state.policy.clone();

    let acc_for_prompt = acc.clone();
    let stop_holder_for_prompt = stop_holder.clone();
    let cwd = state.cwd.clone();
    let prompt = state.prompt.clone();

    async move {
        Client
            .builder()
            .name("luft")
            .on_receive_notification(
                {
                    let acc = acc.clone();
                    let events = events.clone();
                    let activity_tx = activity_tx.clone();
                    let submit_signal = submit_signal.clone();
                    move |n: SessionNotification, _cx: ConnectionTo<Agent>| {
                        let acc = acc.clone();
                        let events = events.clone();
                        let activity_tx = activity_tx.clone();
                        let submit_signal = submit_signal.clone();
                        async move {
                            handle_session_update(
                                n,
                                &acc,
                                &events,
                                &activity_tx,
                                &submit_signal,
                                run_id,
                                agent_id,
                                emit_raw,
                            );
                            Ok(())
                        }
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                {
                    let policy = policy.clone();
                    move |req: RequestPermissionRequest,
                          responder: Responder<RequestPermissionResponse>,
                          _conn: ConnectionTo<Agent>| {
                        let policy = policy.clone();
                        async move { decide_permission(req, responder, policy).await }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(transport, move |conn: ConnectionTo<Agent>| {
                let acc_for_prompt = acc_for_prompt.clone();
                let stop_holder_for_prompt = stop_holder_for_prompt.clone();
                let model = model.clone();
                let luft_binary = luft_binary.clone();
                async move {
                    run_handshake_and_prompt(HandshakePromptContext {
                        conn: &conn,
                        cwd: &cwd,
                        schema_file_path: schema_file_path.as_deref(),
                        luft_binary: luft_binary.as_deref(),
                        model: model.as_deref(),
                        prompt: &prompt,
                        acc: &acc_for_prompt,
                        stop_holder: &stop_holder_for_prompt,
                        backend_id,
                    })
                    .await?;
                    Ok(())
                }
            })
            .await
    }
}

/// Notification handler body. Updates the accumulator, emits a progress event,
/// and signals the watchdog the moment a `workflow_validate_schema` payload is first
/// captured (None → Some transition).
#[allow(clippy::too_many_arguments)]
fn handle_session_update(
    n: SessionNotification,
    acc: &Arc<update_mapper::Accumulator>,
    events: &EventSender,
    activity_tx: &tokio::sync::mpsc::UnboundedSender<()>,
    submit_signal: &Arc<tokio::sync::Notify>,
    run_id: RunId,
    agent_id: AgentId,
    emit_raw: bool,
) {
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

    // Capture pre-update submission state so we can detect the None → Some
    // transition on `workflow_validate_schema` and signal the idle watchdog to switch
    // to a short, non-resetting timer.
    let was_submitted = acc.workflow_validate_schema.lock().unwrap().is_some();
    update_mapper::handle_update(&n.update, run_id, agent_id, acc, events, emit_raw);
    if !was_submitted && acc.workflow_validate_schema.lock().unwrap().is_some() {
        submit_signal.notify_one();
        tracing::debug!(
            "ACP workflow_validate_schema captured; watchdog switching to post-submission mode"
        );
    }
}

/// Permission-request decision. Approves via the task's [`ToolPolicy`]
/// (falling back to approve when no policy is set), then selects the first
/// offered option — `request_permission` is non-interactive in v0.1.
async fn decide_permission(
    req: RequestPermissionRequest,
    responder: Responder<RequestPermissionResponse>,
    policy: Option<ToolPolicy>,
) -> Result<(), agent_client_protocol::Error> {
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
        (true, Some(opt)) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
            opt.option_id.clone(),
        )),
        _ => RequestPermissionOutcome::Cancelled,
    };
    responder.respond(RequestPermissionResponse::new(outcome))
}

// ─── Phase 5: handshake + prompt ────────────────────────────────────────────

/// Drive the ACP handshake: `initialize` → `session/new` (with optional
/// schema-MCP server) → `session/set_config_option` (if a model is requested
/// and the agent advertises one) → `session/prompt`. Records the resulting
/// `StopReason` and token usage into the shared state.
struct HandshakePromptContext<'a> {
    conn: &'a ConnectionTo<Agent>,
    cwd: &'a std::path::Path,
    schema_file_path: Option<&'a str>,
    luft_binary: Option<&'a std::path::Path>,
    model: Option<&'a str>,
    prompt: &'a str,
    acc: &'a Arc<update_mapper::Accumulator>,
    stop_holder: &'a Arc<Mutex<Option<String>>>,
    /// Registry key of this backend (`config.id`), captured into
    /// [`CurrentBackend::id`] during the handshake.
    backend_id: &'a str,
}

async fn run_handshake_and_prompt(
    ctx: HandshakePromptContext<'_>,
) -> Result<(), agent_client_protocol::Error> {
    tracing::debug!("ACP handshake: initialize");
    let init = ctx
        .conn
        .send_request(
            InitializeRequest::new(ProtocolVersion::V1).client_info(
                Implementation::new("luft", env!("CARGO_PKG_VERSION"))
                    .title("Luft"),
            ),
        )
        .block_task()
        .await?;

    // Capture the connected backend's identity into the process-global
    // store, so downstream code can resolve "the current backend" without
    // the adapter having to thread it through every call site.
    match init.agent_info.as_ref() {
        Some(info) => {
            tracing::info!(
                backend = %info.name,
                version = %info.version,
                title = ?info.title,
                "ACP handshake: connected backend"
            );
            luft_core::contract::set_current_backend(luft_core::contract::CurrentBackend {
                id: ctx.backend_id.to_string(),
                name: info.name.clone(),
                version: info.version.clone(),
                title: info.title.clone(),
                client: luft_core::contract::ClientIdentity {
                    name: "luft".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    title: Some("Luft".to_string()),
                },
            });
        }
        None => {
            tracing::warn!("ACP handshake: backend did not report agent_info");
        }
    }

    tracing::debug!("ACP handshake: session/new");
    let ns = session_new(
        ctx.conn,
        ctx.cwd.to_path_buf(),
        ctx.schema_file_path,
        ctx.luft_binary,
    )
    .await?;

    if let Some(model_name) = ctx.model {
        validate_and_set_model(ctx.conn, &ns, model_name).await?;
    }

    tracing::debug!("ACP handshake: session/prompt");
    let pr = send_prompt(ctx.conn, ns.session_id, ctx.prompt.to_string()).await?;
    record_prompt_result(&pr, ctx.stop_holder, ctx.acc);
    Ok(())
}

/// Send `session/new`, attaching the structured-output MCP server when a
/// schema file path is present.
///
/// `luft_binary` overrides which binary gets spawned for
/// `mcp-structured-output`; `None` falls back to `current_exe()` (see
/// `AcpConfig::luft_binary`'s doc comment for when the override is needed).
async fn session_new(
    conn: &ConnectionTo<Agent>,
    cwd: PathBuf,
    schema_file_path: Option<&str>,
    luft_binary: Option<&std::path::Path>,
) -> Result<NewSessionResponse, agent_client_protocol::Error> {
    let req = NewSessionRequest::new(cwd);
    let req = match schema_file_path {
        Some(sf) => {
            let luft_bin = luft_binary
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| {
                    std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("luft"))
                });
            let mcp = McpServerStdio::new("luft-structured-output", luft_bin).args(vec![
                "mcp-structured-output".to_string(),
                "--schema-file".to_string(),
                sf.to_string(),
            ]);
            req.mcp_servers(vec![McpServer::Stdio(mcp)])
        }
        None => req,
    };
    conn.send_request(req).block_task().await
}

/// Validate the requested `model_name` against the agent's advertised
/// `config_options`. If it's listed (either in the ungrouped or grouped
/// select options), emit `session/set_config_option`; otherwise log a warning
/// and fall back to the agent default.
async fn validate_and_set_model(
    conn: &ConnectionTo<Agent>,
    ns: &NewSessionResponse,
    model_name: &str,
) -> Result<(), agent_client_protocol::Error> {
    let config_options = match ns.config_options.as_ref() {
        Some(opts) => opts,
        None => {
            tracing::debug!("ACP: agent does not advertise config_options");
            return Ok(());
        }
    };
    let model_option = match config_options
        .iter()
        .find(|opt| opt.category.as_ref() == Some(&SessionConfigOptionCategory::Model))
    {
        Some(o) => o,
        None => {
            tracing::debug!("ACP: agent does not support model selection");
            return Ok(());
        }
    };
    let select = match &model_option.kind {
        SessionConfigKind::Select(s) => s,
        _ => {
            tracing::debug!("ACP: model option is not a Select kind");
            return Ok(());
        }
    };
    let valid = match &select.options {
        SessionConfigSelectOptions::Ungrouped(opts) => {
            opts.iter().any(|o| o.value.0.as_ref() == model_name)
        }
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .any(|g| g.options.iter().any(|o| o.value.0.as_ref() == model_name)),
        _ => false,
    };
    if valid {
        tracing::debug!(model = %model_name, "ACP: setting session model");
        let req = SetSessionConfigOptionRequest::new(
            ns.session_id.clone(),
            model_option.id.clone(),
            model_name.to_string(),
        );
        conn.send_request(req).block_task().await?;
    } else {
        tracing::warn!(
            model = %model_name,
            "ACP: requested model not available, using agent default"
        );
    }
    Ok(())
}

/// Send `session/prompt` and return the agent's response. Does **not** record
/// the result into shared state — that's [`record_prompt_result`]'s job, so
/// that the recording logic can be unit-tested without an ACP connection.
async fn send_prompt(
    conn: &ConnectionTo<Agent>,
    session_id: SessionId,
    prompt: String,
) -> Result<agent_client_protocol::schema::PromptResponse, agent_client_protocol::Error> {
    conn.send_request(PromptRequest::new(
        session_id,
        vec![ContentBlock::Text(TextContent::new(prompt))],
    ))
    .block_task()
    .await
}

/// Persist a `PromptResponse` into shared state: the `StopReason` is stored
/// as a stable string via [`stop_reason_as_str`]; token usage is folded into
/// the accumulator.
fn record_prompt_result(
    pr: &agent_client_protocol::schema::PromptResponse,
    stop_holder: &Arc<Mutex<Option<String>>>,
    #[cfg_attr(
        not(feature = "unstable_end_turn_token_usage"),
        allow(unused_variables)
    )]
    acc: &Arc<update_mapper::Accumulator>,
) {
    tracing::debug!(stop_reason = ?pr.stop_reason, "ACP prompt complete");
    *stop_holder.lock().unwrap() = Some(stop_reason_as_str(&pr.stop_reason));
    #[cfg(feature = "unstable_end_turn_token_usage")]
    {
        if let Some(u) = pr.usage.as_ref() {
            tracing::debug!(
                input = u.input_tokens,
                output = u.output_tokens,
                total = u.total_tokens,
                "ACP prompt usage"
            );
            *acc.tokens.lock().unwrap() = TokenUsage {
                input: u.input_tokens,
                output: u.output_tokens,
                cache_read: u.cached_read_tokens.unwrap_or(0),
                cache_write: u.cached_write_tokens.unwrap_or(0),
            };
        }
    }
}

/// Convert an ACP [`StopReason`] into a stable PascalCase string. This is the
/// **single source of truth** for the spelling used in [`stop_holder`] and in
/// the synthesized post-submission timeout value ([`STOP_REASON_END_TURN`]).
/// Renaming a variant or adding a new one forces an intentional decision
/// here rather than silently relying on the derived `Debug` format.
fn stop_reason_as_str(r: &StopReason) -> String {
    match r {
        StopReason::EndTurn => STOP_REASON_END_TURN.to_string(),
        StopReason::MaxTokens => "MaxTokens".to_string(),
        StopReason::MaxTurnRequests => "MaxTurnRequests".to_string(),
        StopReason::Refusal => "Refusal".to_string(),
        StopReason::Cancelled => "Cancelled".to_string(),
        #[allow(unreachable_patterns)]
        other => format!("{other:?}"),
    }
}

// ─── Phase 6: race / classify / collect ────────────────────────────────────

/// Convert a `WatchdogOutcome` into a `BackendError` (pre-timeouts) or update
/// the shared `stop_holder` with a synthesized `EndTurn` when the LLM has
/// already submitted its `workflow_validate_schema` (post-submission timeout).
fn handle_watchdog_outcome(
    res: WatchdogOutcome,
    child: &mut tokio::process::Child,
    stop_holder: &Arc<Mutex<Option<String>>>,
    idle_timeout: Duration,
) -> Result<(), BackendError> {
    let _ = child.start_kill();
    match res {
        WatchdogOutcome::PreIdleTimeout => {
            tracing::warn!(
                idle_timeout_ms = idle_timeout.as_millis() as u64,
                "ACP session idle timeout (no protocol activity)"
            );
            Err(BackendError::Timeout)
        }
        WatchdogOutcome::ChannelClosed => {
            tracing::debug!("ACP activity channel closed");
            Err(BackendError::Timeout)
        }
        WatchdogOutcome::PostSubmissionTimeout => {
            // The LLM submitted a valid `workflow_validate_schema` but kept
            // generating tool calls after that (e.g. `todo_write`). We treat
            // the captured submission as the session result: synthesize an
            // `EndTurn` stop_reason and fall through to the normal collection
            // path. The scheduler's schema-retry loop will then validate the
            // payload against `task.output_schema`.
            tracing::info!(
                post_idle_ms = POST_SUBMISSION_IDLE.as_millis() as u64,
                "ACP post-submission timeout; treating workflow_validate_schema as result"
            );
            // Single `MutexGuard` (no double-lock): the original code took
            // the lock twice in a row; we hold it once and only overwrite
            // when the slot is empty so we don't clobber a stop_reason that
            // the connection already wrote.
            let mut guard = stop_holder.lock().unwrap();
            if guard.is_none() {
                *guard = Some(STOP_REASON_END_TURN.to_string());
            }
            Ok(())
        }
    }
}

/// Classify a connection/protocol error: substring-match for upstream "closed"
/// indicators → `Protocol("connection closed")`; everything else becomes a
/// generic `Protocol` error.
///
/// NOTE: this classification relies on upstream `Display` / `to_string()`
/// output. The `agent-client-protocol` crate does not currently expose typed
/// "connection closed" variants, so we pattern-match the rendered strings.
/// If that crate ever surfaces a typed variant, replace this with a `match`
/// on the error type instead of the substrings.
fn classify_protocol_error(e: agent_client_protocol::Error) -> BackendError {
    let s = e.to_string();
    if is_connection_closed(&s) {
        tracing::warn!("ACP connection closed");
        BackendError::Protocol("connection closed".into())
    } else {
        tracing::error!(error = %s, "ACP protocol error");
        BackendError::Protocol(s)
    }
}

fn is_connection_closed(s: &str) -> bool {
    // NOTE: see `classify_protocol_error` for why we pattern-match rendered
    // strings instead of typed error variants. If the ACP crate exposes
    // typed "closed" indicators, migrate to those.
    s.contains("receiver dropped")
        || s.contains("broken pipe")
        || s.contains("unexpected eof")
        || s.contains("connection closed")
}

/// Build the final [`AgentResult`] from the accumulator and stop-reason slot.
fn collect_session_result(task: &AgentTask, state: &SessionState) -> AgentResult {
    let stop = state.stop_holder.lock().unwrap().take().unwrap_or_default();
    let message = std::mem::take(&mut *state.acc.message.lock().unwrap());
    let tokens = *state.acc.tokens.lock().unwrap();
    let structured = state.acc.workflow_validate_schema.lock().unwrap().take();
    result_collector::collect(task, &stop, message, tokens, structured)
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
/// bounds the wait once the LLM has submitted a valid `workflow_validate_schema`
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
            // `workflow_validate_schema` is the result.
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── session/new MCP server wiring ────────────────────────────────
    //
    // The structured-output MCP server reaches the agent only as JSON on the
    // wire. These assert the serialized `session/new` params actually carry
    // it — a silently-empty `mcpServers` array is indistinguishable from
    // "schema validation is off" from the agent's side, and produces a
    // plain-text fallback result that looks like a model failure rather than
    // a wiring bug.

    #[test]
    fn session_new_request_serializes_mcp_server() {
        let mcp =
            McpServerStdio::new("luft-structured-output", PathBuf::from("luft.exe")).args(vec![
                "mcp-structured-output".to_string(),
                "--schema-file".to_string(),
                "/tmp/schema.json".to_string(),
            ]);
        let req =
            NewSessionRequest::new(PathBuf::from("/work")).mcp_servers(vec![McpServer::Stdio(mcp)]);

        let json = serde_json::to_value(&req).expect("request serializes");
        let servers = json
            .get("mcpServers")
            .and_then(|v| v.as_array())
            .expect("mcpServers must be present as an array");
        assert_eq!(servers.len(), 1, "serialized as: {json}");
        assert_eq!(servers[0]["name"], "luft-structured-output");
        assert_eq!(servers[0]["command"], "luft.exe");
        assert_eq!(servers[0]["args"][0], "mcp-structured-output");
        assert_eq!(servers[0]["args"][1], "--schema-file");
        // Per the ACP schema, the `Stdio` variant of `McpServer` is
        // `#[serde(untagged)]` — unlike `Http`/`Sse`, a stdio server carries
        // NO `"type"` discriminator on the wire. Asserted explicitly because
        // "the tag is missing" looks like a serialization bug otherwise.
        assert!(
            servers[0].get("type").is_none(),
            "stdio MCP servers must serialize untagged (no `type` key): {json}"
        );
    }

    #[test]
    fn session_new_request_without_schema_has_empty_mcp_servers() {
        // The no-schema path must NOT invent a server entry.
        let req = NewSessionRequest::new(PathBuf::from("/work"));
        let json = serde_json::to_value(&req).expect("request serializes");
        let servers = json.get("mcpServers").and_then(|v| v.as_array());
        assert!(
            servers.is_none_or(|s| s.is_empty()),
            "expected no servers, got: {json}"
        );
    }

    // ── skill_dirs_for_backend / write_workflow_skill_files ──────────
    #[test]
    fn skill_dirs_for_known_backends() {
        assert_eq!(skill_dirs_for_backend("codex"), &[".agents/skills"]);
        assert_eq!(skill_dirs_for_backend("opencode"), &[".agents/skills"]);
        assert_eq!(skill_dirs_for_backend("claude"), &[".claude/skills"]);
        assert_eq!(skill_dirs_for_backend("claude-code"), &[".claude/skills"]);
    }

    #[test]
    fn skill_dirs_for_unknown_backend_is_empty() {
        let dirs: &[&str] = skill_dirs_for_backend("some-future-backend");
        assert!(dirs.is_empty());
    }

    #[test]
    fn write_to_dir_writes_main_and_references() {
        let tmp = tempfile::tempdir().unwrap();
        let skill = luft_core::contract::skill::Skill {
            name: "sample",
            description: "d",
            content: "# main body",
            references: &[("references/extra.md", "extra content")],
        };
        let dir = tmp.path().join("workflow");
        let count = luft_skills::write_to_dir(&dir, &skill).unwrap();
        assert_eq!(count, 2);

        let written = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
        assert!(written.starts_with("---\nname: sample\ndescription: d\n---\n\n"));
        assert!(written.ends_with("# main body"));
        assert_eq!(
            std::fs::read_to_string(dir.join("references/extra.md")).unwrap(),
            "extra content"
        );
    }

    #[test]
    fn write_workflow_skill_files_only_writes_the_matching_backend_dir() {
        let tmp = tempfile::tempdir().unwrap();
        write_workflow_skill_files("codex", tmp.path());

        assert!(tmp.path().join(".agents/skills/workflow/SKILL.md").exists());
        // Only codex's own convention gets written — not the others.
        assert!(!tmp.path().join(".claude/skills/workflow/SKILL.md").exists());
    }

    #[test]
    fn write_workflow_skill_files_unknown_backend_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        write_workflow_skill_files("mock", tmp.path());
        assert!(!tmp.path().join(".agents").exists());
        assert!(!tmp.path().join(".claude").exists());
    }

    #[test]
    fn write_workflow_skill_files_content_matches_workflow_skill() {
        let tmp = tempfile::tempdir().unwrap();
        write_workflow_skill_files("codex", tmp.path());
        let written =
            std::fs::read_to_string(tmp.path().join(".agents/skills/workflow/SKILL.md")).unwrap();
        assert!(written.starts_with("---\nname: workflow\n"));
        assert!(written.contains(luft_skills::WORKFLOW_SKILL.content));
        // A reference file also made it to disk.
        let ref_dir = tmp.path().join(".agents/skills/workflow/references");
        assert!(ref_dir.is_dir());
        assert!(std::fs::read_dir(&ref_dir).unwrap().count() > 0);
    }

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
    // calling `todo_write` after submitting `workflow_validate_schema`) cannot
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
                Duration::from_secs(60),   // long pre-idle (should never fire)
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

    // ── stop_reason_as_str (F5 / F6) ────────────────────────────────
    //
    // The persisted stop_reason string is consumed by
    // `result_collector::status_from_stop_reason`, which substring-matches
    // on the PascalCase Debug-derived spelling. We pin the spelling here
    // so that any future change to the helper is intentional.

    #[test]
    fn stop_reason_as_str_end_turn_matches_constant() {
        assert_eq!(
            stop_reason_as_str(&StopReason::EndTurn),
            STOP_REASON_END_TURN
        );
        assert_eq!(stop_reason_as_str(&StopReason::EndTurn), "EndTurn");
    }

    #[test]
    fn stop_reason_as_str_cancelled_contains_cancel() {
        assert_eq!(stop_reason_as_str(&StopReason::Cancelled), "Cancelled");
    }

    #[test]
    fn stop_reason_as_str_other_variants_stable() {
        assert_eq!(stop_reason_as_str(&StopReason::MaxTokens), "MaxTokens");
        assert_eq!(
            stop_reason_as_str(&StopReason::MaxTurnRequests),
            "MaxTurnRequests"
        );
        assert_eq!(stop_reason_as_str(&StopReason::Refusal), "Refusal");
    }

    // ── handle_watchdog_outcome (F7) ────────────────────────────────
    //
    // Post-submission timeout synthesizes an EndTurn stop_reason via a
    // single MutexGuard (no double-lock) and only when the slot is empty.

    #[tokio::test]
    async fn handle_watchdog_post_submission_synthesizes_end_turn() {
        let stop: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        // We don't have a real child here; use a dummy. We only care that
        // `start_kill` doesn't panic on a process we never spawned.
        let mut child = match tokio::process::Command::new("cmd")
            .arg("/C")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return, // non-Windows CI: skip
        };
        let r = handle_watchdog_outcome(
            WatchdogOutcome::PostSubmissionTimeout,
            &mut child,
            &stop,
            Duration::from_secs(300),
        );
        assert!(
            r.is_ok(),
            "post-submission outcome should fall through to collect"
        );
        assert_eq!(stop.lock().unwrap().as_deref(), Some("EndTurn"));
    }

    #[tokio::test]
    async fn handle_watchdog_post_submission_preserves_existing_stop() {
        let stop: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(Some("Cancelled".into())));
        let mut child = match tokio::process::Command::new("cmd")
            .arg("/C")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        let r = handle_watchdog_outcome(
            WatchdogOutcome::PostSubmissionTimeout,
            &mut child,
            &stop,
            Duration::from_secs(300),
        );
        assert!(r.is_ok());
        // Must not clobber a stop reason the connection already wrote.
        assert_eq!(stop.lock().unwrap().as_deref(), Some("Cancelled"));
    }

    #[tokio::test]
    async fn handle_watchdog_pre_idle_returns_timeout_error() {
        let stop: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let mut child = match tokio::process::Command::new("cmd")
            .arg("/C")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        let r = handle_watchdog_outcome(
            WatchdogOutcome::PreIdleTimeout,
            &mut child,
            &stop,
            Duration::from_secs(1),
        );
        assert!(matches!(r, Err(BackendError::Timeout)));
    }

    #[tokio::test]
    async fn handle_watchdog_channel_closed_returns_timeout_error() {
        let stop: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let mut child = match tokio::process::Command::new("cmd")
            .arg("/C")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        let r = handle_watchdog_outcome(
            WatchdogOutcome::ChannelClosed,
            &mut child,
            &stop,
            Duration::from_secs(1),
        );
        assert!(matches!(r, Err(BackendError::Timeout)));
    }

    // ── is_connection_closed (F4) ───────────────────────────────────

    #[test]
    fn is_connection_closed_matches_documented_substrings() {
        assert!(is_connection_closed("receiver dropped"));
        assert!(is_connection_closed("broken pipe"));
        assert!(is_connection_closed("unexpected eof"));
        assert!(is_connection_closed("connection closed"));
        assert!(is_connection_closed(
            "io error: broken pipe writing to stdin"
        ));
        assert!(!is_connection_closed("unknown protocol method"));
        assert!(!is_connection_closed(""));
    }
}
