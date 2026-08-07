//! # luft-daemon
//!
//! Daemon process crate — WebSocket server hosting all workflow execution.
//!
//! The daemon listens on `127.0.0.1:{port}` and serves:
//! - `GET /` — web dashboard (HTML/CSS/JS) for monitoring runs
//! - `GET /api/*` — REST API for run data (JSON)
//! - WS `/mcp` — MCP JSON-RPC (proxied stdio ↔ WS by `luft mcp serve`)
//! - WS `/run` — run protocol (used by `luft run`)
//!
//! WebSocket connections are auto-detected via the `Upgrade: websocket`
//! header; all other HTTP requests are served by the dashboard.
//!
//! Discovery via PID file at `$LUFT_HOME/daemon.pid` or `~/.luft/daemon.pid`.

pub mod autostart;
pub mod dashboard;
pub mod dashboard_assets;
pub mod process;
pub mod run_session;
pub mod server;

pub use autostart::discover_or_autostart;
pub use process::{discover, is_alive, read as read_pid, PidFile};
pub use server::serve;
