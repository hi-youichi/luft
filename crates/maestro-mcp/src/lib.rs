//! # maestro-mcp
//!
//! MCP (Model Context Protocol) server crate for Maestro.
//!
//! Exposes workflow authoring resources and execution tools to external
//! AI clients via a stdio JSON-RPC transport.
//!
//! ## Resources
//! - `workflow://schema` — embedded Lua DSL reference (markdown)
//! - `workflow://examples` — dynamic list of example workflows (JSON)
//! - `workflow://example/{name}` — read a specific example `.lua` file
//!
//! ## Tools
//! - `execute_workflow` — validate + fire-and-forget execute a Lua workflow
//! - `list_workflows` — list available `.lua` files
//! - `get_run_status` — query a run's checkpoint status
//! - `get_run_events` — query a run's event log
//!
//! ## Usage
//!
//! The server is started via the CLI (`maestro mcp serve`) or directly:
//!
//! ```no_run
//! use maestro_mcp::McpServer;
//! use std::time::Duration;
//! use maestro_core::{MockBackend, MockBehavior, TokenUsage};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let backend = MockBackend::new("mock", vec![MockBehavior::Success {
//!     output: serde_json::json!({}),
//!     tokens: TokenUsage::default(),
//!     delay: Duration::ZERO,
//! }]);
//! let maestro = maestro::Maestro::builder()
//!     .backend(backend)
//!     .build()?;
//! let server = McpServer::new(maestro);
//! server.serve_stdio().await?;
//! # Ok(())
//! # }
//! ```

pub mod protocol;
pub mod resources;
pub mod server;
pub mod tools;

pub use protocol::{
    error_codes, initialize_result, resource_templates_list_result, resources_list_result,
    tools_list_result, JsonRpcError, JsonRpcMessage, JsonRpcResponse, RpcError,
};
pub use resources::{build_read_response, list_examples, read_resource, ResourceContent, WorkflowUri};
pub use server::McpServer;
pub use tools::{handle_call, new_run_registry, RunInfo, RunRegistry};
