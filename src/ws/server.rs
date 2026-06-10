use crate::core::contract::backend::AgentBackend;
use crate::ws::handler::AppState;
use crate::ws::registry::RunRegistry;
use crate::ws::config::ServeConfig;

use axum::Router;
use axum::routing::get;
use std::sync::Arc;

pub async fn serve(
    config: ServeConfig,
    backend: Arc<dyn AgentBackend>,
) -> Result<(), anyhow::Error> {
    let state = AppState {
        backend,
        registry: RunRegistry::default(),
        base_dir: config.base_dir,
        run_permits: Arc::new(tokio::sync::Semaphore::new(config.max_concurrent_runs)),
        confirm_timeout: config.confirm_timeout,
    };

    let app = Router::new()
        .route("/ws", get(crate::ws::handler::ws_handler))
        .route("/health", get(crate::ws::handler::health_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.addr).await?;
    tracing::info!("maestro ws server listening on {}", config.addr);

    axum::serve(listener, app).await?;

    Ok(())
}
