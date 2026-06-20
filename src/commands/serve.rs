//! `serve` subcommand: start the WebSocket server, auto-detecting the backend
//! if one isn't specified.

use super::runs_base_dir;
use crate::backend;
use anyhow::Result;

pub async fn serve_cmd(
    addr: String,
    backend_id: Option<String>,
    max_concurrent: usize,
    no_acp_raw: bool,
) -> Result<()> {
    let backend_id = backend_id.unwrap_or_else(|| backend::detect_backend().to_string());
    let backend = backend::create_backend(&backend_id, !no_acp_raw)?;
    let config = maestro::ws::ServeConfig {
        addr: addr
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid address {:?}: {}", addr, e))?,
        base_dir: runs_base_dir(),
        max_concurrent_runs: max_concurrent,
        confirm_timeout: std::time::Duration::from_secs(30),
    };
    eprintln!("maestro ws server on {} (backend: {})", config.addr, backend_id);
    maestro::ws::serve(config, backend).await
}
