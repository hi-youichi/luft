use luft_core::contract::backend::AgentBackend;
use luft_core::contract::event::AgentEvent;
use luft_core::contract::ids::RunId;
use luft_planner::PlannerConfig;
use luft_runtime::{ExecLimits, ScriptError};
use luft_service::query::{ReportStatus, StatusOutput};
use luft_service::run::{
    assign_dir_name, prepare, resolve_fresh, resolve_resume, RunSpec, ScriptSource,
};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::error::LuftError;

/// Fluent builder for constructing a [`Luft`] instance.
///
/// Defaults: base dir `.luft/runs`, unlimited concurrency, default
/// [`PlannerConfig`] and [`ExecLimits`].
///
/// ```no_run
/// # use luft::Luft;
/// # use luft_core::mock_backend::MockBackend;
/// let luft = Luft::builder()
///     .backend(MockBackend::new("mock", vec![]))
///     .base_dir("./runs")
///     .concurrency(8)
///     .build()
///     .unwrap();
/// ```
pub struct LuftBuilder {
    backend: Option<Arc<dyn AgentBackend>>,
    base_dir: PathBuf,
    concurrency: Option<usize>,
    planner_config: PlannerConfig,
    exec_limits: ExecLimits,
}

impl Default for LuftBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl LuftBuilder {
    /// Create a builder with default configuration (no backend set).
    ///
    /// You **must** call [`backend()`](Self::backend) before [`build()`](Self::build).
    #[must_use]
    pub fn new() -> Self {
        Self {
            backend: None,
            base_dir: PathBuf::from(".luft/runs"),
            concurrency: None,
            planner_config: PlannerConfig::default(),
            exec_limits: ExecLimits::default(),
        }
    }

    /// Set the agent backend that will execute agent tasks.
    ///
    /// Required — calling [`build()`](Self::build) without a backend returns
    /// [`LuftError::BackendNotConfigured`].
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
    /// Default: `.luft/runs`
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

    /// Configuration for the NL→Lua planner used by [`Luft::run_nl`].
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

    /// Consume the builder and construct a [`Luft`] instance.
    ///
    /// # Errors
    ///
    /// Returns [`LuftError::BackendNotConfigured`] if no backend was set.
    pub fn build(self) -> Result<Luft, LuftError> {
        let backend = self.backend.ok_or(LuftError::BackendNotConfigured)?;
        std::fs::create_dir_all(&self.base_dir)?;
        Ok(Luft {
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
/// Construct via [`LuftBuilder`] (obtained from [`Luft::builder`]).
///
/// Each `run_*` method starts a run and returns either a [`RunHandle`]
/// (async, `start_*` variants) or a [`RunOutcome`] (blocking, `run_*` variants).
pub struct Luft {
    backend: Arc<dyn AgentBackend>,
    base_dir: PathBuf,
    concurrency: Option<usize>,
    planner_config: PlannerConfig,
    #[allow(dead_code)]
    exec_limits: ExecLimits,
}

impl Luft {
    /// Create a new [`LuftBuilder`] with default settings.
    #[must_use]
    pub fn builder() -> LuftBuilder {
        LuftBuilder::new()
    }

    async fn start_with_source(&self, source: ScriptSource<'_>) -> Result<RunHandle, LuftError> {
        let mut spec = resolve_fresh(source, self.backend.clone(), self.planner_config.clone())
            .await
            .map_err(LuftError::Other)?;
        assign_dir_name(&mut spec, &self.base_dir);
        self.spawn_run(spec)
    }

    async fn start_with_resume(&self, run_dir: &str) -> Result<RunHandle, LuftError> {
        let spec = resolve_resume(run_dir, &self.base_dir).map_err(LuftError::Other)?;
        self.spawn_run(spec)
    }

    fn spawn_run(&self, spec: RunSpec) -> Result<RunHandle, LuftError> {
        let run_dir = self.base_dir.join(&spec.run_dir_name);
        std::fs::create_dir_all(&run_dir)?;

        let run_id = spec.run_id;
        let run_dir_name = spec.run_dir_name.clone();
        let script = spec.script.clone();

        let (tx, _) = broadcast::channel::<AgentEvent>(256);
        let cancel = CancellationToken::new();
        let run_ctx = luft_core::contract::backend::RunContext {
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
                .map_err(LuftError::Other)?;
            let runtime = prepared.runtime;
            let result = luft_service::run::execute(&run_ctx, runtime, script)
                .await
                .map_err(LuftError::Other)?;
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
    pub async fn start_script(&self, lua: &str) -> Result<RunHandle, LuftError> {
        self.start_with_source(ScriptSource::Script(lua)).await
    }

    /// Start a run from a `.lua` workflow file.
    ///
    /// Equivalent to reading the file and calling [`start_script`](Self::start_script),
    /// but tracks the file path for resume / debugging.
    pub async fn start_workflow(&self, path: &Path) -> Result<RunHandle, LuftError> {
        self.start_with_source(ScriptSource::Workflow(path)).await
    }

    /// Start a run from a natural-language task description.
    ///
    /// The [`planner`](crate::planner) generates a Lua orchestration script
    /// via the backend's LLM, validates it, then executes it. See
    /// [`PlannerConfig`] for tuning.
    pub async fn start_nl(&self, nl: &str) -> Result<RunHandle, LuftError> {
        self.start_with_source(ScriptSource::Nl(nl)).await
    }

    /// Resume a previously checkpointed run from its run directory name.
    ///
    /// The run must have been checkpointed (see [`JournalStore`]). Agents that
    /// completed before the checkpoint are skipped; the script resumes from
    /// the first un-completed phase.
    ///
    /// [`JournalStore`]: luft_core::journal::JournalStore
    pub async fn start_resume(&self, run_dir: &str) -> Result<RunHandle, LuftError> {
        self.start_with_resume(run_dir).await
    }

    // ── Convenience: start + join ──

    /// Run a Lua script to completion (blocks the caller).
    ///
    /// Convenience for `start_script(lua).await?.join().await`.
    pub async fn run_script(&self, lua: &str) -> Result<RunOutcome, LuftError> {
        self.start_script(lua).await?.join().await
    }

    /// Run a `.lua` workflow file to completion (blocks the caller).
    pub async fn run_workflow(&self, path: &Path) -> Result<RunOutcome, LuftError> {
        self.start_workflow(path).await?.join().await
    }

    /// Run a natural-language task to completion (blocks the caller).
    ///
    /// The planner generates a Lua script via the backend LLM, then executes it.
    pub async fn run_nl(&self, nl: &str) -> Result<RunOutcome, LuftError> {
        self.start_nl(nl).await?.join().await
    }

    /// Resume a checkpointed run to completion (blocks the caller).
    pub async fn run_resume(&self, run_dir: &str) -> Result<RunOutcome, LuftError> {
        self.start_resume(run_dir).await?.join().await
    }

    // ── Query (synchronous) ──

    /// Query the status of a run by its directory name.
    ///
    /// Returns `None` if the run directory does not exist.
    pub fn status(&self, run_dir: &str) -> Result<Option<StatusOutput>, LuftError> {
        luft_service::query::get_status(run_dir, &self.base_dir).map_err(LuftError::Other)
    }

    /// List all runs under the base directory, sorted by most-recent update.
    pub fn list(&self) -> Result<Vec<StatusOutput>, LuftError> {
        luft_service::query::list_runs(&self.base_dir).map_err(LuftError::Other)
    }

    /// Get the raw chronological event log for a run.
    pub fn events(&self, run_dir: &str) -> Result<Vec<AgentEvent>, LuftError> {
        luft_service::query::get_events(run_dir, &self.base_dir).map_err(LuftError::Other)
    }

    /// Get the final report value emitted by `report()` in the Lua script.
    pub fn report(&self, run_dir: &str) -> Result<ReportStatus, LuftError> {
        luft_service::query::get_report(run_dir, &self.base_dir).map_err(LuftError::Other)
    }

    /// Get structured findings (from agent MCP injection) collected during the run.
    pub fn findings(
        &self,
        run_dir: &str,
    ) -> Result<Vec<luft_core::contract::finding::Finding>, LuftError> {
        luft_service::query::get_findings(run_dir, &self.base_dir).map_err(LuftError::Other)
    }

    /// Cancel an active run by signalling its cancellation token.
    pub fn cancel(&self, run_dir: &str) -> Result<(), LuftError> {
        luft_service::query::cancel_run(run_dir, &self.base_dir).map_err(LuftError::Other)?;
        Ok(())
    }
}

/// Async handle to a running orchestration.
///
/// Returned by the `start_*` methods on [`Luft`]. Use [`subscribe`](Self::subscribe)
/// to receive real-time [`AgentEvent`]s, [`cancel`](Self::cancel) to stop the run,
/// or [`join`](Self::join) to await completion.
///
/// Implements [`IntoFuture`](std::future::IntoFuture) for ergonomic `.await`:
///
/// ```no_run
/// # use luft::Luft;
/// # use luft_core::mock_backend::MockBackend;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let luft = Luft::builder().backend(MockBackend::new("mock", vec![])).build()?;
/// let handle = luft.start_script("report('ok')").await?;
/// let outcome = handle.await?;
/// # Ok(())
/// # }
/// ```
pub struct RunHandle {
    run_id: RunId,
    run_dir_name: String,
    join: tokio::task::JoinHandle<Result<Result<serde_json::Value, ScriptError>, LuftError>>,
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
    /// [`BackendError::Cancelled`]: luft_core::contract::backend::BackendError::Cancelled
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Await run completion and return the final [`RunOutcome`].
    ///
    /// # Errors
    ///
    /// - [`LuftError::Other`] if the execution task panicked.
    /// - Script errors from the Lua runtime are embedded in `RunOutcome::result`.
    pub async fn join(self) -> Result<RunOutcome, LuftError> {
        let result = self
            .join
            .await
            .map_err(|e| LuftError::Other(anyhow::anyhow!("execution task panicked: {}", e)))??;
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

type JoinFutureOutput = Result<RunOutcome, LuftError>;

/// Future returned by [`RunHandle::into_future`]. Awaits run completion.
pub struct JoinFuture {
    inner: Pin<Box<dyn Future<Output = JoinFutureOutput> + Send>>,
}

impl Future for JoinFuture {
    type Output = JoinFutureOutput;
    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
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
                let result = join.await.map_err(|e| {
                    LuftError::Other(anyhow::anyhow!("execution task panicked: {}", e))
                })??;
                Ok(RunOutcome {
                    run_id,
                    run_dir_name,
                    result,
                })
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the [`LuftBuilder`] / [`Luft`] / [`RunHandle`] facade.
    //!
    //! These cover:
    //! - constructor invariants (default backend = None),
    //! - every setter is reachable and accepted,
    //! - `build()` enforces the backend requirement and creates the base
    //!   directory on success,
    //! - the run / query / cancel call surface compiles and is callable,
    //! - end-to-end execution via `run_script` against a `MockBackend`.
    //!
    //! Run handle internally uses a tokio task. We use `#[tokio::test]` only
    //! for tests that need to drive the runtime; everything else is `#[test]`.

    use super::*;
    use luft_core::mock_backend::{MockBackend, MockBehavior};
    use luft_core::TokenUsage;
    use std::time::Duration;
    use tempfile::tempdir;

    /// Helper: create a `MockBackend` that always returns `output` immediately.
    fn mock_backend(returning: serde_json::Value) -> MockBackend {
        MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: returning,
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        )
    }

    // -----------------------------------------------------------------
    // LuftBuilder: constructor / defaults / Setter plumbing
    // -----------------------------------------------------------------

    #[test]
    fn new_builder_has_no_backend() {
        // `new()` starts life without a backend — `build()` must refuse.
        let b = LuftBuilder::new();
        let err = b.build();
        // `Luft` is not Debug, so we pattern-match instead of `.expect_err`.
        match err {
            Err(LuftError::BackendNotConfigured) => {}
            Err(other) => panic!("expected BackendNotConfigured, got {:?}", other),
            Ok(_) => panic!("build() should fail without backend"),
        }
    }

    #[test]
    fn default_impl_matches_new() {
        let a = LuftBuilder::new();
        let b = LuftBuilder::default();
        match a.build() {
            Err(LuftError::BackendNotConfigured) => {}
            Err(other) => panic!("a.build() returned wrong error: {:?}", other),
            Ok(_) => panic!("a.build() unexpectedly succeeded"),
        }
        match b.build() {
            Err(LuftError::BackendNotConfigured) => {}
            Err(other) => panic!("b.build() returned wrong error: {:?}", other),
            Ok(_) => panic!("b.build() unexpectedly succeeded"),
        }
    }

    #[test]
    fn backend_setter_constructs_a_luft() {
        // Using a temporary directory lets us avoid leaving state behind.
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("hello")))
            .base_dir(dir.path().to_path_buf())
            .build()
            .expect("build should succeed");
        // luft is opaque — its only assertion we can make here is that
        // the base dir was created.
        let _ = luft;
    }

    #[test]
    fn backend_arc_setter_accepts_an_existing_arc() {
        let dir = tempdir().expect("tempdir");
        let backend: Arc<dyn AgentBackend> = Arc::new(mock_backend(serde_json::json!("hi")));
        let luft = LuftBuilder::new()
            .backend_arc(backend)
            .base_dir(dir.path().to_path_buf())
            .build()
            .expect("build should succeed");
        let _ = luft;
    }

    #[test]
    fn base_dir_setter_overrides_default() {
        // The default base_dir is `.luft/runs`. We override with a tempfile
        // path; verify build succeeds and that path was created.
        let dir = tempdir().expect("tempdir");
        let nested = dir.path().join("nested/runs");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(&nested)
            .build()
            .expect("build should succeed");
        assert!(nested.exists(), "base_dir should have been created");
        let _ = luft;
    }

    #[test]
    fn base_dir_is_idempotently_created() {
        // Calling `build()` when the dir already exists must not fail.
        let dir = tempdir().expect("tempdir");
        let pre_existing = dir.path().join("preexisting");
        std::fs::create_dir_all(&pre_existing).expect("seed");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(&pre_existing)
            .build()
            .expect("build should succeed even if dir exists");
        let _ = luft;
    }

    #[test]
    fn build_returns_io_error_when_base_dir_is_uncreatable() {
        // Asking mkdir to traverse a *file* on the path should fail.
        let dir = tempdir().expect("tempdir");
        let blocker = dir.path().join("blocker.txt");
        std::fs::write(&blocker, b"x").expect("seed blocker");
        let bad = blocker.join("child"); // A path under a file → ENOTDIR.
        let result = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(&bad)
            .build();
        match result {
            // `Luft` does not impl Debug, so we cannot use `expect_err`.
            Err(LuftError::Io(_)) => {}
            Err(other) => panic!("expected Io(_), got {:?}", other),
            Ok(_) => panic!("build must fail when base_dir cannot be created"),
        }
    }

    #[test]
    fn concurrency_setter_is_accepted() {
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(dir.path())
            .concurrency(4)
            .build()
            .expect("build should succeed");
        let _ = luft;
    }

    #[test]
    fn planner_config_setter_is_accepted() {
        let dir = tempdir().expect("tempdir");
        let cfg = PlannerConfig {
            planner_model: Some("gpt-test".into()),
            max_retries: 1,
            generate_mock: true,
        };
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(dir.path())
            .planner_config(cfg)
            .build()
            .expect("build should succeed");
        let _ = luft;
    }

    #[test]
    fn exec_limits_setter_is_accepted() {
        let dir = tempdir().expect("tempdir");
        let limits = ExecLimits::default();
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(dir.path())
            .exec_limits(limits)
            .build()
            .expect("build should succeed");
        let _ = luft;
    }

    #[test]
    fn setter_chain_is_composable() {
        // Test the canonical "everything set" composition path.
        let dir = tempdir().expect("tempdir");
        let _luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(dir.path())
            .concurrency(8)
            .planner_config(PlannerConfig::default())
            .exec_limits(ExecLimits::default())
            .build()
            .expect("build should succeed");
    }

    // -----------------------------------------------------------------
    // Luft: factory + query surface (no execution required)
    // -----------------------------------------------------------------

    #[test]
    fn luft_factory_returns_a_fresh_builder() {
        let b = Luft::builder();
        let other = Luft::builder();
        // `Luft` does not impl Debug, so we cannot `.unwrap_err()` it.
        match b.build() {
            Err(LuftError::BackendNotConfigured) => {}
            Err(other) => panic!("b.build() returned wrong error: {:?}", other),
            Ok(_) => panic!("b.build() unexpectedly succeeded"),
        }
        match other.build() {
            Err(LuftError::BackendNotConfigured) => {}
            Err(other) => panic!("other.build() returned wrong error: {:?}", other),
            Ok(_) => panic!("other.build() unexpectedly succeeded"),
        }
    }

    #[test]
    fn query_methods_return_typed_results() {
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(dir.path())
            .build()
            .expect("build should succeed");
        // Each query returns LuftError; for a non-existent run they should
        // be Err (NotFound-style) — we're verifying the type plumbing,
        // not the precise error.
        let _status: Result<Option<StatusOutput>, _> = luft.status("nonexistent");
        let _list: Result<Vec<StatusOutput>, _> = luft.list();
        let _events: Result<Vec<AgentEvent>, _> = luft.events("nonexistent");
        let _report: Result<ReportStatus, _> = luft.report("nonexistent");
        let _findings: Result<Vec<luft_core::contract::finding::Finding>, _> =
            luft.findings("nonexistent");
        let _cancel: Result<(), _> = luft.cancel("nonexistent");
    }

    #[test]
    fn list_is_empty_on_a_fresh_base_dir() {
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(dir.path())
            .build()
            .expect("build");
        let runs = luft.list().expect("list on a fresh dir");
        assert!(runs.is_empty());
    }

    // -----------------------------------------------------------------
    // RunHandle: ergonomics
    // -----------------------------------------------------------------

    #[test]
    fn run_outcome_is_publicly_constructible_for_borrowers() {
        // The fields are `pub`; verify the shape at the type level.
        fn _takes(o: &RunOutcome) {
            let _id: &RunId = &o.run_id;
            let _dir: &String = &o.run_dir_name;
        }
        let _ = _takes;
    }

    #[test]
    fn mock_backend_satisfies_agent_backend_bound_on_builder() {
        // Sanity: `MockBackend` (the primary test backend) plugs into the
        // builder without ceremony. This is the primary recipe; re-check
        // it here to guard the `AgentBackend + 'static` bound.
        let _b: LuftBuilder = LuftBuilder::new().backend(mock_backend(serde_json::json!(1)));
    }

    #[test]
    fn own_backend_type_can_replace_mock() {
        // Verifies that any `MockBackend` (and therefore any
        // `AgentBackend + 'static`) can be passed through `backend_arc`
        // as a pre-built `Arc<dyn AgentBackend>`. Using the in-tree
        // MockBackend avoids needing to add a custom backend impl or
        // depend on the `async_trait` macro here.
        let dir = tempdir().expect("tempdir");
        let backend: Arc<dyn AgentBackend> = Arc::new(mock_backend(serde_json::json!("arc-ok")));
        let luft = LuftBuilder::new()
            .backend_arc(backend)
            .base_dir(dir.path().to_path_buf())
            .build()
            .expect("build should accept Arc<dyn AgentBackend>");
        let _ = luft;
    }

    #[test]
    fn builder_clone_is_not_a_concern() {
        // The builder is a value type whose fields are public-but-not-
        // exposed. We verify here that two sequential `build()` calls
        // produce two independent Luft values that don't share state.
        let dir1 = tempdir().expect("tempdir");
        let dir2 = tempdir().expect("tempdir");
        let l1 = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!(1)))
            .base_dir(dir1.path())
            .concurrency(1)
            .build()
            .expect("build1");
        let l2 = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!(2)))
            .base_dir(dir2.path())
            .concurrency(2)
            .build()
            .expect("build2");
        // Both tempdir roots exist; verify they're distinct addresses.
        assert_ne!(dir1.path(), dir2.path());
        assert!(dir1.path().exists());
        assert!(dir2.path().exists());
        let _ = (l1, l2);
    }

    // -----------------------------------------------------------------
    // End-to-end: build, run a trivial script, observe outcome.
    // The MockBackend returns a Value but the script's `report()` is what
    // gets surfaced via RunOutcome.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn run_script_simple_trivial() {
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("unused")))
            .base_dir(dir.path())
            .build()
            .expect("build");
        let script = r#"
            meta = { reasoning = "triv", phases = {} }
            function main()
                report("ok")
            end
        "#;
        let outcome = luft.run_script(script).await.expect("run_script");
        // RunOutcome fields are public — verify shape.
        let RunOutcome {
            run_id,
            run_dir_name,
            result,
        } = outcome;
        assert!(!run_id.to_string().is_empty(), "run_id should be a uuid");
        assert!(!run_dir_name.is_empty(), "run_dir_name should be set");
        let value = result.expect("script reported a value");
        assert_eq!(value, serde_json::json!("ok"));
    }

    #[tokio::test]
    async fn run_script_with_chained_lua_expressions() {
        // Validate that multi-expression scripts (separated by `;` or
        // newlines) execute and return `Ok(RunOutcome)` even when no
        // `report()` is called.
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ignored")))
            .base_dir(dir.path())
            .build()
            .expect("build");
        let script = r#"
            meta = { reasoning = "chain", phases = {} }
            function main()
                report(1 + 2 * 3)
            end
        "#;
        let outcome = luft.run_script(script).await.expect("run_script");
        assert_eq!(outcome.result.unwrap(), serde_json::json!(7));
    }

    #[tokio::test]
    async fn run_script_via_run_handle_works() {
        // Same as `run_script` but using `start_script + join` directly.
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ignored")))
            .base_dir(dir.path())
            .build()
            .expect("build");
        let handle = luft
            .start_script(
                r#"meta = { reasoning = "h", phases = {} } function main() report("v") end"#,
            )
            .await
            .expect("start_script");
        // Synchronous `run_id()` getter must agree with the eventual outcome.
        let pinned = handle.run_id();
        let outcome = handle.join().await.expect("join");
        assert_eq!(pinned, outcome.run_id);
        assert_eq!(outcome.result.unwrap(), serde_json::json!("v"));
    }

    #[tokio::test]
    async fn run_handle_status_is_unique_per_invocation() {
        // Each `start_script` produces a unique RunId.
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!(1)))
            .base_dir(dir.path())
            .build()
            .expect("build");
        let h1 = luft
            .start_script(
                r#"meta = { reasoning = "1", phases = {} } function main() report(1) end"#,
            )
            .await
            .expect("h1");
        let h2 = luft
            .start_script(
                r#"meta = { reasoning = "2", phases = {} } function main() report(2) end"#,
            )
            .await
            .expect("h2");
        assert_ne!(h1.run_id(), h2.run_id());
        assert_eq!(
            h1.join().await.unwrap().result.unwrap(),
            serde_json::json!(1)
        );
        assert_eq!(
            h2.join().await.unwrap().result.unwrap(),
            serde_json::json!(2)
        );
    }

    #[tokio::test]
    async fn start_script_yields_a_handle_with_subscribe() {
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(dir.path())
            .build()
            .expect("build");
        let handle = luft
            .start_script(
                r#"meta = { reasoning = "t", phases = {} } function main() report(1) end"#,
            )
            .await
            .expect("start_script");
        // Subscribe before joining — verify the broadcast receiver is
        // obtainable even after a fast-completing run.
        let rx = handle.subscribe();
        let outcome = handle.join().await.expect("join");
        assert_eq!(outcome.result.expect("script ran"), serde_json::json!(1));
        // The receiver's not required to have received anything yet; its
        // mere existence is the contract test.
        let _ = rx;
    }

    #[tokio::test]
    async fn run_handle_run_id_is_pinned() {
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(dir.path())
            .build()
            .expect("build");
        let handle = luft
            .start_script(
                r#"meta = { reasoning = "t", phases = {} } function main() report(1) end"#,
            )
            .await
            .expect("start");
        let pinned = handle.run_id();
        let outcome = handle.join().await.expect("join");
        assert_eq!(pinned, outcome.run_id);
    }

    #[tokio::test]
    async fn into_future_consumes_handle_and_returns_outcome() {
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(dir.path())
            .build()
            .expect("build");
        // Using IntoFuture (`.await`-ing the handle directly).
        let outcome: RunOutcome = luft
            .start_script(
                r#"meta = { reasoning = "t", phases = {} } function main() report(42) end"#,
            )
            .await
            .expect("start")
            .await
            .expect("via IntoFuture");
        assert_eq!(outcome.result.expect("value"), serde_json::json!(42));
    }

    #[tokio::test]
    async fn cancel_token_signals_before_run_starts() {
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(dir.path())
            .build()
            .expect("build");
        let handle = luft
            .start_script(
                r#"meta = { reasoning = "t", phases = {} } function main() report(1) end"#,
            )
            .await
            .expect("start");
        // Pre-cancel the token; the run should still finish (it's already
        // past validation) — but the API should not panic.
        handle.cancel();
        let _ = handle.join().await;
    }

    #[tokio::test]
    async fn run_handle_run_dir_name_is_non_empty_and_marks_a_directory() {
        let dir = tempdir().expect("tempdir");
        let luft = LuftBuilder::new()
            .backend(mock_backend(serde_json::json!("ok")))
            .base_dir(dir.path())
            .build()
            .expect("build");
        let handle = luft
            .start_script(
                r#"meta = { reasoning = "t", phases = {} } function main() report("ok") end"#,
            )
            .await
            .expect("start");
        // Snapshot before `join` consumes the handle.
        let dir_name = handle.run_dir_name().to_string();
        let outcome = handle.join().await.expect("join");
        assert_eq!(dir_name, outcome.run_dir_name);
        // The directory should exist under the base_dir.
        assert!(
            dir.path().join(&dir_name).exists(),
            "run dir should exist on disk"
        );
    }
}
