//! # luft-mcp
//!
//! MCP (Model Context Protocol) server crate for Luft.
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
//! - `execute_workflow` — validate + fire-and-forget execute (or resume) a Lua workflow
//! - `list_files` — list available `.lua` files
//! - `list_runs` — paginated history of past runs
//! - `get_run_status` — rich run status (phases/agents/report/error)
//! - `get_run_events` — paginated/filtered run event log
//! - `cancel_run` — cancel an in-flight run
//!
//! `run_id` is the run directory name itself throughout — there is no
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

pub mod protocol;
pub mod resources;
pub mod server;
pub mod server_rmcp;
pub mod tools;

pub use protocol::{
    error_codes, initialize_result, resource_templates_list_result, resources_list_result,
    tools_list_result, JsonRpcError, JsonRpcMessage, JsonRpcResponse, RpcError,
};
pub use resources::{
    build_read_response, list_examples, read_resource, ResourceContent, WorkflowUri,
};
pub use server::McpServer;
pub use server_rmcp::{serve as serve_rmcp, LuftMcpServer};
pub use tools::handle_call;

#[cfg(test)]
mod tests {
    //! Tests for the crate root re-exports.
    //!
    //! `lib.rs` is a pure re-export module. The tests below serve as a
    //! compile-time check that the public API surface is intact and that all
    //! re-exported items remain reachable through the crate root.

    use super::*;
    use serde_json::Value;
    use std::path::PathBuf;

    // ── Protocol type re-exports ────────────────────────────────────────

    #[test]
    fn protocol_types_are_constructible_via_reexport() {
        // Build each re-exported type using its public constructor — if the
        // re-export breaks, this stops compiling.
        let _msg = JsonRpcMessage {
            jsonrpc: "2.0".into(),
            id: Some(Value::from(1)),
            method: Some("ping".into()),
            params: None,
        };
        let resp = JsonRpcResponse::new(Value::from(2), serde_json::json!({"ok": true}));
        assert_eq!(resp.jsonrpc, "2.0");
        let err = JsonRpcError::new(Value::from(3), -32601, "method not found");
        assert_eq!(err.error.code, -32601);
        assert_eq!(err.error.message, "method not found");
    }

    #[test]
    fn protocol_serde_roundtrip_through_reexport() {
        // JsonRpcMessage deserializes through the re-exported path.
        let msg: JsonRpcMessage =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).unwrap();
        assert_eq!(msg.method.as_deref(), Some("ping"));
    }

    #[test]
    fn error_codes_constants_accessible() {
        assert_eq!(error_codes::PARSE_ERROR, -32700);
        assert_eq!(error_codes::INVALID_PARAMS, -32602);
        assert_eq!(error_codes::METHOD_NOT_FOUND, -32601);
        assert_eq!(error_codes::INTERNAL_ERROR, -32603);
    }

    #[test]
    fn initialize_result_through_reexport() {
        let r = initialize_result();
        assert_eq!(r["protocolVersion"], "2024-11-05");
        assert_eq!(r["serverInfo"]["name"], "luft");
    }

    #[test]
    fn resources_list_result_through_reexport() {
        let r = resources_list_result();
        let resources = r["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 2);
    }

    #[test]
    fn resource_templates_list_result_through_reexport() {
        let r = resource_templates_list_result();
        assert_eq!(r["resourceTemplates"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn tools_list_result_through_reexport() {
        let r = tools_list_result();
        assert_eq!(r["tools"].as_array().unwrap().len(), 6);
    }

    // ── Resources re-exports ────────────────────────────────────────────

    #[test]
    fn workflow_uri_variants_via_reexport() {
        // Each variant is constructible through the re-exported type.
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
        // Smoke-test the re-exported free function against the schema resource.
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

    // ── Tools re-exports ────────────────────────────────────────────────

    #[tokio::test]
    async fn handle_call_unknown_tool_via_reexport() {
        // Calling handle_call directly via the re-export with a bogus tool name
        // must produce an isError response.
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

        let params = serde_json::json!({ "name": "nope", "arguments": {} });
        let result = handle_call(&params, &luft, &[]).await;
        assert_eq!(result["isError"], true);
    }

    // ── McpServer re-export ─────────────────────────────────────────────

    #[tokio::test]
    async fn mcp_server_constructible_via_reexport() {
        // Compile-time + construction-time check that McpServer is reachable.
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

        let server = McpServer::new(luft).search_dirs(vec![PathBuf::from("/tmp")]);
        // Exercise a known method to confirm wiring is intact.
        let r = server
            .dispatch_method("ping", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(r.is_object());
    }

    // ── API-surface completeness check ──────────────────────────────────

    /// Compile-time assertion that every item listed in the re-export `pub use`
    /// declarations is reachable through the crate root.
    ///
    /// If any of these `use` statements stop resolving (because the re-export
    /// changed), this test will fail to compile.
    #[test]
    fn public_api_surface_is_complete() {
        fn _assert_protocol_in_scope() {
            let _: fn() -> serde_json::Value = initialize_result;
            let _: fn() -> serde_json::Value = resources_list_result;
            let _: fn() -> serde_json::Value = resource_templates_list_result;
            let _: fn() -> serde_json::Value = tools_list_result;
        }
        fn _assert_resources_in_scope() {
            let _: fn(&WorkflowUri, &[PathBuf]) -> anyhow::Result<ResourceContent> = read_resource;
            let _: fn(&str, &[PathBuf]) -> anyhow::Result<Value> = build_read_response;
            let _: fn(&[PathBuf]) -> Vec<crate::resources::ExampleEntry> = list_examples;
        }
        fn _assert_tools_in_scope() {
            // `handle_call` is async; just proving the name resolves through
            // the re-export is enough (its exact signature is exercised by
            // the tests that actually call it).
            let _ = handle_call;
        }
        // All checks above are type-system assertions — pass at compile time.
    }

    // ── Module declarations are reachable ───────────────────────────────

    #[test]
    fn submodules_are_declared_and_usable() {
        // Reaching into the modules via crate-relative paths proves the
        // `pub mod` declarations above are still present.
        fn _check_protocol_module(_: crate::protocol::JsonRpcResponse) {}
        fn _check_resources_module(_: crate::resources::WorkflowUri) {}
        fn _check_server_module(_: McpServer) {}
        fn _check_tools_module() {
            let _: fn(&[luft_core::contract::event::AgentEvent], &str) -> Vec<_> =
                crate::tools::filter_events_since;
        }
    }
}
