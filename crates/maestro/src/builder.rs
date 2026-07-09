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
    pub fn new() -> Self {
        Self {
            backend: None,
            base_dir: PathBuf::from(".maestro/runs"),
            concurrency: None,
            planner_config: PlannerConfig::default(),
            exec_limits: ExecLimits::default(),
        }
    }

    pub fn backend<B: AgentBackend + 'static>(mut self, b: B) -> Self {
        self.backend = Some(Arc::new(b));
        self
    }

    pub fn base_dir<P: Into<PathBuf>>(mut self, dir: P) -> Self {
        self.base_dir = dir.into();
        self
    }

    pub fn concurrency(mut self, n: usize) -> Self {
        self.concurrency = Some(n);
        self
    }

    pub fn planner_config(mut self, cfg: PlannerConfig) -> Self {
        self.planner_config = cfg;
        self
    }

    pub fn exec_limits(mut self, limits: ExecLimits) -> Self {
        self.exec_limits = limits;
        self
    }

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

pub struct Maestro {
    backend: Arc<dyn AgentBackend>,
    base_dir: PathBuf,
    concurrency: Option<usize>,
    planner_config: PlannerConfig,
    #[allow(dead_code)]
    exec_limits: ExecLimits,
}

impl Maestro {
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

    pub async fn start_script(&self, lua: &str) -> Result<RunHandle, MaestroError> {
        self.start_with_source(ScriptSource::Script(lua)).await
    }

    pub async fn start_workflow(&self, path: &Path) -> Result<RunHandle, MaestroError> {
        self.start_with_source(ScriptSource::Workflow(path)).await
    }

    pub async fn start_nl(&self, nl: &str) -> Result<RunHandle, MaestroError> {
        self.start_with_source(ScriptSource::Nl(nl)).await
    }

    pub async fn start_resume(&self, run_dir: &str) -> Result<RunHandle, MaestroError> {
        self.start_with_resume(run_dir).await
    }

    // ── Convenience: start + join ──

    pub async fn run_script(&self, lua: &str) -> Result<RunOutcome, MaestroError> {
        self.start_script(lua).await?.join().await
    }

    pub async fn run_workflow(&self, path: &Path) -> Result<RunOutcome, MaestroError> {
        self.start_workflow(path).await?.join().await
    }

    pub async fn run_nl(&self, nl: &str) -> Result<RunOutcome, MaestroError> {
        self.start_nl(nl).await?.join().await
    }

    pub async fn run_resume(&self, run_dir: &str) -> Result<RunOutcome, MaestroError> {
        self.start_resume(run_dir).await?.join().await
    }

    // ── Query (synchronous) ──

    pub fn status(&self, run_dir: &str) -> Result<Option<StatusOutput>, MaestroError> {
        maestro_service::query::get_status(run_dir, &self.base_dir)
            .map_err(MaestroError::Other)
    }

    pub fn list(&self) -> Result<Vec<StatusOutput>, MaestroError> {
        maestro_service::query::list_runs(&self.base_dir)
            .map_err(MaestroError::Other)
    }

    pub fn events(&self, run_dir: &str) -> Result<Vec<AgentEvent>, MaestroError> {
        maestro_service::query::get_events(run_dir, &self.base_dir)
            .map_err(MaestroError::Other)
    }

    pub fn report(&self, run_dir: &str) -> Result<ReportStatus, MaestroError> {
        maestro_service::query::get_report(run_dir, &self.base_dir)
            .map_err(MaestroError::Other)
    }

    pub fn findings(&self, run_dir: &str) -> Result<Vec<maestro_core::contract::finding::Finding>, MaestroError> {
        maestro_service::query::get_findings(run_dir, &self.base_dir)
            .map_err(MaestroError::Other)
    }

    pub fn cancel(&self, run_dir: &str) -> Result<(), MaestroError> {
        maestro_service::query::cancel_run(run_dir, &self.base_dir)
            .map_err(MaestroError::Other)?;
        Ok(())
    }
}

pub struct RunHandle {
    run_id: RunId,
    run_dir_name: String,
    join: tokio::task::JoinHandle<Result<Result<serde_json::Value, ScriptError>, MaestroError>>,
    cancel: CancellationToken,
    events: broadcast::Sender<AgentEvent>,
}

impl RunHandle {
    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    pub fn run_dir_name(&self) -> &str {
        &self.run_dir_name
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

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

pub struct RunOutcome {
    pub run_id: RunId,
    pub run_dir_name: String,
    pub result: Result<serde_json::Value, ScriptError>,
}

type JoinFutureOutput = Result<RunOutcome, MaestroError>;

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
