use maestro_core::contract::backend::AgentBackend;
use maestro_core::contract::event::AgentEvent;
use maestro_core::contract::ids::RunId;
use maestro_planner::PlannerConfig;
use maestro_runtime::{ExecLimits, ScriptError};
use maestro_service::query::{ReportStatus, StatusOutput};
use maestro_service::run::{
    assign_dir_name, prepare, resolve_fresh, resolve_resume, ScriptSource, RunSpec,
};
use std::future::Future;
use std::pin::Pin;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::error::MaestroError;

/// Fluent builder for constructing a [`Maestro`] instance.
///
/// Defaults: base dir `.maestro/runs`, unlimited concurrency, default
/// [`PlannerConfig`] and [`ExecLimits`].
///
/// ```no_run
/// # use maestro::Maestro;
/// # use maestro_core::mock_backend::MockBackend;
/// let maestro = Maestro::builder()
///     .backend(MockBackend::new("mock", vec![]))
///     .base_dir("./runs")
///     .concurrency(8)
///     .build()
///     .unwrap();
/// ```
pub struct MaestroBuilder {
    backend: Option<Arc<dyn AgentBackend>>,
    base_dir: PathBuf,
    concurrency: Option<usize>,
    planner_config: PlannerConfig,
    exec_limits: ExecLimits,
}

impl Default for MaestroBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MaestroBuilder {
    /// Create a builder with default configuration (no backend set).
    ///
    /// You **must** call [`backend()`](Self::backend) before [`build()`](Self::build).
    #[must_use]
    pub fn new() -> Self {
        Self {
            backend: None,
            base_dir: PathBuf::from(".maestro/runs"),
            concurrency: None,
            planner_config: PlannerConfig::default(),
            exec_limits: ExecLimits::default(),
        }
    }

    /// Set the agent backend that will execute agent tasks.
    ///
    /// Required — calling [`build()`](Self::build) without a backend returns
    /// [`MaestroError::BackendNotConfigured`].
    #[must_use]
    pub fn backend<B: AgentBackend + 'static>(mut self, b: B) -> Self {
        self.backend = Some(Arc::new(b));
        self
    }

    /// Like [`backend`](Self::backend) but accepts an already-`Arc`ed backend.
    ///
    /// Useful when the backend is constructed by a factory that returns
    /// `Arc<dyn AgentBackend>` (e.g. the CLI backend factory).
    #[must_use]
    pub fn backend_arc(mut self, b: Arc<dyn AgentBackend>) -> Self {
        self.backend = Some(b);
        self
    }

    /// Directory where run artifacts (checkpoints, event logs, SQLite DB)
    /// are stored. Created on [`build()`](Self::build) if it does not exist.
    ///
    /// Default: `.maestro/runs`
    #[must_use]
    pub fn base_dir<P: Into<PathBuf>>(mut self, dir: P) -> Self {
        self.base_dir = dir.into();
        self
    }

    /// Maximum number of agent tasks that may run concurrently.
    ///
    /// `None` (default) = no explicit limit, letting the scheduler pick.
    #[must_use]
    pub fn concurrency(mut self, n: usize) -> Self {
        self.concurrency = Some(n);
        self
    }

    /// Configuration for the NL→Lua planner used by [`Maestro::run_nl`].
    #[must_use]
    pub fn planner_config(mut self, cfg: PlannerConfig) -> Self {
        self.planner_config = cfg;
        self
    }

    /// Execution limits (instruction budget, memory caps) applied to the
    /// Lua sandbox.
    #[must_use]
    pub fn exec_limits(mut self, limits: ExecLimits) -> Self {
        self.exec_limits = limits;
        self
    }

    /// Consume the builder and construct a [`Maestro`] instance.
    ///
    /// # Errors
    ///
    /// Returns [`MaestroError::BackendNotConfigured`] if no backend was set.
    pub fn build(self) -> Result<Maestro, MaestroError> {
        let backend = self.backend.ok_or(MaestroError::BackendNotConfigured)?;
        std::fs::create_dir_all(&self.base_dir)?;
        Ok(Maestro {
            backend,
            base_dir: self.base_dir,
            concurrency: self.concurrency,
            planner_config: self.planner_config,
            exec_limits: self.exec_limits,
        })
    }
}

/// Top-level orchestrator. Entry point for running Lua orchestration scripts.
///
/// Construct via [`MaestroBuilder`] (obtained from [`Maestro::builder`]).
///
/// Each `run_*` method starts a run and returns either a [`RunHandle`]
/// (async, `start_*` variants) or a [`RunOutcome`] (blocking, `run_*` variants).
pub struct Maestro {
    backend: Arc<dyn AgentBackend>,
    base_dir: PathBuf,
    concurrency: Option<usize>,
    planner_config: PlannerConfig,
    #[allow(dead_code)]
    exec_limits: ExecLimits,
}

impl Maestro {
    /// Create a new [`MaestroBuilder`] with default settings.
    #[must_use]
    pub fn builder() -> MaestroBuilder {
        MaestroBuilder::new()
    }

    async fn start_with_source(
        &self,
        source: ScriptSource<'_>,
    ) -> Result<RunHandle, MaestroError> {
        let mut spec = resolve_fresh(source, self.backend.clone(), self.planner_config.clone())
            .await
            .map_err(MaestroError::Other)?;
        assign_dir_name(&mut spec, &self.base_dir);
        self.spawn_run(spec)
    }

    async fn start_with_resume(&self, run_dir: &str) -> Result<RunHandle, MaestroError> {
        let spec = resolve_resume(run_dir, &self.base_dir)
            .map_err(MaestroError::Other)?;
        self.spawn_run(spec)
    }

    fn spawn_run(&self, spec: RunSpec) -> Result<RunHandle, MaestroError> {
        let run_dir = self.base_dir.join(&spec.run_dir_name);
        std::fs::create_dir_all(&run_dir)?;

        let run_id = spec.run_id;
        let run_dir_name = spec.run_dir_name.clone();
        let script = spec.script.clone();

        let (tx, _) = broadcast::channel::<AgentEvent>(256);
        let cancel = CancellationToken::new();
        let run_ctx = maestro_core::contract::backend::RunContext {
            run_id,
            cancel: cancel.clone(),
            events: tx.clone(),
        };

        let backend = self.backend.clone();
        let base_dir = self.base_dir.clone();
        let concurrency = self.concurrency;

        let join = tokio::spawn(async move {
            let prepared = prepare(&spec, backend, &base_dir, &run_ctx, concurrency)
                .await
                .map_err(MaestroError::Other)?;
            let runtime = prepared.runtime;
            let result = maestro_service::run::execute(&run_ctx, runtime, script)
                .await
                .map_err(MaestroError::Other)?;
            Ok(result)
        });

        Ok(RunHandle {
            run_id,
            run_dir_name,
            join,
            cancel,
            events: tx,
        })
    }

    // ── Async execution: returns RunHandle ──

    /// Start a run from a raw Lua script string. Returns immediately with a
    /// [`RunHandle`] for async fire-and-forget execution.
    ///
    /// Use [`run_script`](Self::run_script) for the blocking convenience variant.
    pub async fn start_script(&self, lua: &str) -> Result<RunHandle, MaestroError> {
        self.start_with_source(ScriptSource::Script(lua)).await
    }

    /// Start a run from a `.lua` workflow file.
    ///
    /// Equivalent to reading the file and calling [`start_script`](Self::start_script),
    /// but tracks the file path for resume / debugging.
    pub async fn start_workflow(&self, path: &Path) -> Result<RunHandle, MaestroError> {
        self.start_with_source(ScriptSource::Workflow(path)).await
    }

    /// Start a run from a natural-language task description.
    ///
    /// The [`planner`](crate::planner) generates a Lua orchestration script
    /// via the backend's LLM, validates it, then executes it. See
    /// [`PlannerConfig`] for tuning.
    pub async fn start_nl(&self, nl: &str) -> Result<RunHandle, MaestroError> {
        self.start_with_source(ScriptSource::Nl(nl)).await
    }

    /// Resume a previously checkpointed run from its run directory name.
    ///
    /// The run must have been checkpointed (see [`JournalStore`]). Agents that
    /// completed before the checkpoint are skipped; the script resumes from
    /// the first un-completed phase.
    ///
    /// [`JournalStore`]: maestro_core::journal::JournalStore
    pub async fn start_resume(&self, run_dir: &str) -> Result<RunHandle, MaestroError> {
        self.start_with_resume(run_dir).await
    }

    // ── Convenience: start + join ──

    /// Run a Lua script to completion (blocks the caller).
    ///
    /// Convenience for `start_script(lua).await?.join().await`.
    pub async fn run_script(&self, lua: &str) -> Result<RunOutcome, MaestroError> {
        self.start_script(lua).await?.join().await
    }

    /// Run a `.lua` workflow file to completion (blocks the caller).
    pub async fn run_workflow(&self, path: &Path) -> Result<RunOutcome, MaestroError> {
        self.start_workflow(path).await?.join().await
    }

    /// Run a natural-language task to completion (blocks the caller).
    ///
    /// The planner generates a Lua script via the backend LLM, then executes it.
    pub async fn run_nl(&self, nl: &str) -> Result<RunOutcome, MaestroError> {
        self.start_nl(nl).await?.join().await
    }

    /// Resume a checkpointed run to completion (blocks the caller).
    pub async fn run_resume(&self, run_dir: &str) -> Result<RunOutcome, MaestroError> {
        self.start_resume(run_dir).await?.join().await
    }

    // ── Query (synchronous) ──

    /// Query the status of a run by its directory name.
    ///
    /// Returns `None` if the run directory does not exist.
    pub fn status(&self, run_dir: &str) -> Result<Option<StatusOutput>, MaestroError> {
        maestro_service::query::get_status(run_dir, &self.base_dir)
            .map_err(MaestroError::Other)
    }

    /// List all runs under the base directory, sorted by most-recent update.
    pub fn list(&self) -> Result<Vec<StatusOutput>, MaestroError> {
        maestro_service::query::list_runs(&self.base_dir)
            .map_err(MaestroError::Other)
    }

    /// Get the raw chronological event log for a run.
    pub fn events(&self, run_dir: &str) -> Result<Vec<AgentEvent>, MaestroError> {
        maestro_service::query::get_events(run_dir, &self.base_dir)
            .map_err(MaestroError::Other)
    }

    /// Get the final report value emitted by `report()` in the Lua script.
    pub fn report(&self, run_dir: &str) -> Result<ReportStatus, MaestroError> {
        maestro_service::query::get_report(run_dir, &self.base_dir)
            .map_err(MaestroError::Other)
    }

    /// Get structured findings (from agent MCP injection) collected during the run.
    pub fn findings(&self, run_dir: &str) -> Result<Vec<maestro_core::contract::finding::Finding>, MaestroError> {
        maestro_service::query::get_findings(run_dir, &self.base_dir)
            .map_err(MaestroError::Other)
    }

    /// Cancel an active run by signalling its cancellation token.
    pub fn cancel(&self, run_dir: &str) -> Result<(), MaestroError> {
        maestro_service::query::cancel_run(run_dir, &self.base_dir)
            .map_err(MaestroError::Other)?;
        Ok(())
    }
}

/// Async handle to a running orchestration.
///
/// Returned by the `start_*` methods on [`Maestro`]. Use [`subscribe`](Self::subscribe)
/// to receive real-time [`AgentEvent`]s, [`cancel`](Self::cancel) to stop the run,
/// or [`join`](Self::join) to await completion.
///
/// Implements [`IntoFuture`](std::future::IntoFuture) for ergonomic `.await`:
///
/// ```no_run
/// # use maestro::Maestro;
/// # use maestro_core::mock_backend::MockBackend;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let maestro = Maestro::builder().backend(MockBackend::new("mock", vec![])).build()?;
/// let handle = maestro.start_script("report('ok')").await?;
/// let outcome = handle.await?;
/// # Ok(())
/// # }
/// ```
pub struct RunHandle {
    run_id: RunId,
    run_dir_name: String,
    join: tokio::task::JoinHandle<Result<Result<serde_json::Value, ScriptError>, MaestroError>>,
    cancel: CancellationToken,
    events: broadcast::Sender<AgentEvent>,
}

impl RunHandle {
    /// Unique identifier (UUID v7) for this run.
    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    /// Directory name (relative to base dir) where run artifacts are stored.
    pub fn run_dir_name(&self) -> &str {
        &self.run_dir_name
    }

    /// Subscribe to real-time [`AgentEvent`]s for this run.
    ///
    /// Events are broadcast — multiple subscribers each receive a full copy.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    /// Signal the run's cancellation token. Agents observing the token will
    /// return promptly with [`BackendError::Cancelled`].
    ///
    /// [`BackendError::Cancelled`]: maestro_core::contract::backend::BackendError::Cancelled
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Await run completion and return the final [`RunOutcome`].
    ///
    /// # Errors
    ///
    /// - [`MaestroError::Other`] if the execution task panicked.
    /// - Script errors from the Lua runtime are embedded in `RunOutcome::result`.
    pub async fn join(self) -> Result<RunOutcome, MaestroError> {
        let result = self.join.await
            .map_err(|e| MaestroError::Other(anyhow::anyhow!("execution task panicked: {}", e)))??;
        Ok(RunOutcome {
            run_id: self.run_id,
            run_dir_name: self.run_dir_name,
            result,
        })
    }
}

/// Result of a completed run.
///
/// `result` is `Ok` when the Lua script ran to completion, `Err` if the
/// sandbox reported a script error (timeout, instruction limit, etc.).
pub struct RunOutcome {
    /// Unique identifier (UUID v7) for this run.
    pub run_id: RunId,
    /// Directory name where artifacts (checkpoint, events, SQLite DB) reside.
    pub run_dir_name: String,
    /// The Lua script's final `report()` value (`Ok`) or a script error (`Err`).
    pub result: Result<serde_json::Value, ScriptError>,
}

type JoinFutureOutput = Result<RunOutcome, MaestroError>;

/// Future returned by [`RunHandle::into_future`]. Awaits run completion.
pub struct JoinFuture {
    inner: Pin<Box<dyn Future<Output = JoinFutureOutput> + Send>>,
}

impl Future for JoinFuture {
    type Output = JoinFutureOutput;
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

impl std::future::IntoFuture for RunHandle {
    type IntoFuture = JoinFuture;
    type Output = JoinFutureOutput;

    fn into_future(self) -> Self::IntoFuture {
        let run_id = self.run_id;
        let run_dir_name = self.run_dir_name;
        let join = self.join;
        JoinFuture {
            inner: Box::pin(async move {
                let result = join.await
                    .map_err(|e| MaestroError::Other(anyhow::anyhow!("execution task panicked: {}", e)))??;
                Ok(RunOutcome {
                    run_id,
                    run_dir_name,
                    result,
                })
            }),
        }
    }
}
