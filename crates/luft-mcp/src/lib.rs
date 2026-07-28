//! # luft-mcp
//!
//! MCP (Model Context Protocol) server crate for Luft.
//!
//! Exposes workflow authoring resources and execution tools to external
//! AI clients via stdio transport using the `rmcp` SDK.
//!
//! ## Resources
//! - `workflow://schema` - embedded Lua DSL reference (markdown)
//! - `workflow://examples` - dynamic list of example workflows (JSON)
//! - `workflow://example/{name}` - read a specific example `.lua` file
//!
//! ## Tools
//! - `execute_workflow` - validate + fire-and-forget execute (or resume) a Lua workflow
//! - `list_files` - list available `.lua` files
//! - `list_runs` - paginated history of past runs
//! - `get_run_status` - rich run status (phases/agents/report/error)
//! - `get_run_events` - paginated/filtered run event log
//! - `cancel_run` - cancel an in-flight run
//!
//! `run_id` is the run directory name itself throughout - there is no
//! separate UUID layer.
//!
//! ## Usage
//!
//! The server is started via the CLI (`luft mcp serve`) or directly:
//!
//! ```no_run
//! use luft_mcp::LuftMcpServer;
//! use std::time::Duration;
//! use luft_core::{MockBackend, MockBehavior, TokenUsage};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let backend = MockBackend::new("mock", vec![MockBehavior::Success {
//!     output: serde_json::json!({}),
//!     tokens: TokenUsage::default(),
//!     delay: Duration::ZERO,
//! }]);
//! let luft = luft::Luft::builder()
//!     .backend(backend)
//!     .build()?;
//! let server = LuftMcpServer::new(luft);
//! luft_mcp::serve_rmcp(server).await?;
//! # Ok(())
//! # }
//! ```

pub mod proxy;
pub mod resources;
pub mod server_rmcp;
pub mod ws_transport;

pub use resources::{
    build_read_response, list_examples, read_resource, ResourceContent, WorkflowUri,
};
pub use server_rmcp::{serve as serve_rmcp, LuftMcpServer};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn workflow_uri_variants_via_reexport() {
        assert_eq!(
            WorkflowUri::parse("workflow://schema"),
            Some(WorkflowUri::Schema)
        );
        assert_eq!(
            WorkflowUri::parse("workflow://examples"),
            Some(WorkflowUri::Examples)
        );
        assert_eq!(
            WorkflowUri::parse("workflow://example/hi"),
            Some(WorkflowUri::Example("hi".into()))
        );
        assert!(WorkflowUri::parse("http://nope").is_none());
    }

    #[test]
    fn resource_content_via_reexport() {
        let _c = ResourceContent {
            mime_type: "text/plain",
            text: "hello".to_string(),
        };
    }

    #[test]
    fn read_resource_schema_via_reexport() {
        let content = read_resource(&WorkflowUri::Schema, &[]).unwrap();
        assert!(!content.text.is_empty());
    }

    #[test]
    fn list_examples_empty_via_reexport() {
        let entries = list_examples(&[PathBuf::from("/nonexistent")]);
        assert!(entries.is_empty());
    }

    #[test]
    fn build_read_response_schema_via_reexport() {
        let resp = build_read_response("workflow://schema", &[]).unwrap();
        assert_eq!(resp["contents"][0]["mimeType"], "text/markdown");
    }

    #[tokio::test]
    async fn luft_mcp_server_constructible_via_reexport() {
        use luft_core::{MockBackend, MockBehavior, TokenUsage};
        use std::time::Duration;

        let backend = MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!({}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        );
        let luft = luft::Luft::builder()
            .backend(backend)
            .base_dir(tempfile::TempDir::new().unwrap().keep())
            .build()
            .unwrap();

        let _server = LuftMcpServer::new(luft);
    }
}
