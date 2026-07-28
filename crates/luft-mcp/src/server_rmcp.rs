//! RMCP server — thin MCP transport facade.
//!
//! All business logic lives in `luft_service::WorkflowServiceImpl`.
//! This module only wires rmcp tool handlers to the service and provides
//! the `ServerHandler` / resource implementations.

use luft::Luft;
use luft_service::request::{
    CancelRunRequest, ExecuteWorkflowRequest, GetRunEventsRequest, GetRunStatusRequest,
    ListRunsRequest,
};
use luft_service::{WorkflowServiceImpl, WorkflowService};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

// ── LuftMcpServer ──────────────────────────────────────────────────────

/// RMCP-based MCP server — a thin facade over `WorkflowServiceImpl`.
#[derive(Clone)]
pub struct LuftMcpServer {
    pub service: Arc<WorkflowServiceImpl>,
    tool_router: ToolRouter<Self>,
    client_name: Arc<OnceLock<String>>,
}

impl LuftMcpServer {
    pub fn new(luft: Luft) -> Self {
        let service = Arc::new(WorkflowServiceImpl::new(
            luft,
            vec![PathBuf::from("examples"), PathBuf::from("workflows")],
        ));
        let mut s = Self {
            service,
            tool_router: ToolRouter::default(),
            client_name: Arc::new(OnceLock::new()),
        };
        s.tool_router = Self::tool_router();
        s
    }

    pub fn search_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        Arc::get_mut(&mut self.service)
            .expect("search_dirs must be called before sharing")
            .set_search_dirs(dirs);
        self
    }

    pub fn luft(&self) -> &Luft {
        self.service.luft()
    }

    pub fn client_name(&self) -> Option<&str> {
        self.client_name.get().map(|s| s.as_str())
    }

    pub fn is_codex(&self) -> bool {
        matches!(
            self.client_name(),
            Some(n) if n.eq_ignore_ascii_case("codex")
        )
    }
}

// ── Tools ──────────────────────────────────────────────────────────────

#[tool_router]
impl LuftMcpServer {
    #[tool(
        description = "Execute a Luft workflow, or resume a prior checkpointed run. Exactly one of `script`, `path`, `resume_from_id` is required. Returns immediately with a run_id — use get_run_status to poll progress."
    )]
    async fn execute_workflow(
        &self,
        Parameters(req): Parameters<ExecuteWorkflowRequest>,
    ) -> Result<String, String> {
        let (resp, _handle) = self
            .service
            .start_workflow(req)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }

    #[tool(
        description = "List available .lua workflow files from workflows/ and examples/ directories"
    )]
    fn list_files(&self) -> Result<String, String> {
        let resp = self.service.list_files().map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }

    #[tool(
        description = "List past workflow runs, paginated and optionally filtered by terminal status"
    )]
    fn list_runs(
        &self,
        Parameters(req): Parameters<ListRunsRequest>,
    ) -> Result<String, String> {
        let resp = self.service.list_runs(req).map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Get the current rich status of a workflow run, including per-phase and per-agent detail"
    )]
    fn get_run_status(
        &self,
        Parameters(req): Parameters<GetRunStatusRequest>,
    ) -> Result<String, String> {
        let resp = self
            .service
            .get_run_status(req)
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }

    #[tool(description = "Get paginated/filtered events for a workflow run")]
    fn get_run_events(
        &self,
        Parameters(req): Parameters<GetRunEventsRequest>,
    ) -> Result<String, String> {
        let resp = self
            .service
            .get_run_events(req)
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }

    #[tool(description = "Cancel an in-flight workflow run")]
    fn cancel_run(
        &self,
        Parameters(req): Parameters<CancelRunRequest>,
    ) -> Result<String, String> {
        let resp = self.service.cancel_run(req).map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }
}

// ── Resources + ServerHandler ──────────────────────────────────────────

#[tool_handler]
impl ServerHandler for LuftMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            server_info: Implementation {
                name: "luft".into(),
                title: None,
                version: env!("CARGO_PKG_VERSION").into(),
                icons: None,
                website_url: None,
            },
            instructions: None,
        }
    }

    async fn initialize(
        &self,
        request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        let _ = self.client_name.set(request.client_info.name.clone());
        tracing::info!(
            client = %request.client_info.name,
            version = %request.client_info.version,
            "mcp client connected"
        );
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        Ok(self.get_info())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::with_all_items(vec![
            Annotated::new(
                RawResource {
                    uri: "workflow://schema".into(),
                    name: "Workflow DSL Reference".into(),
                    title: None,
                    description: Some(
                        "Complete Lua DSL syntax for writing Luft workflows".into(),
                    ),
                    mime_type: Some("text/markdown".into()),
                    size: None,
                    icons: None,
                },
                None,
            ),
            Annotated::new(
                RawResource {
                    uri: "workflow://examples".into(),
                    name: "Example Workflows".into(),
                    title: None,
                    description: Some("List of available example workflows".into()),
                    mime_type: Some("application/json".into()),
                    size: None,
                    icons: None,
                },
                None,
            ),
        ]))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParam>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            Annotated::new(
                RawResourceTemplate {
                    uri_template: "workflow://example/{name}".into(),
                    name: "Example Workflow".into(),
                    title: None,
                    description: Some("Read a specific example workflow by name".into()),
                    mime_type: Some("text/x-lua".into()),
                },
                None,
            ),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = &request.uri;
        let parsed = crate::resources::WorkflowUri::parse(uri).ok_or_else(|| {
            McpError::resource_not_found(
                "unknown_uri",
                Some(serde_json::json!({ "uri": uri })),
            )
        })?;

        let content =
            crate::resources::read_resource(&parsed, self.service.search_dirs()).map_err(|e| {
                McpError::internal_error(
                    "read_failed",
                    Some(serde_json::json!({ "error": e.to_string() })),
                )
            })?;

        Ok(ReadResourceResult {
            contents: vec![ResourceContents::TextResourceContents {
                uri: uri.clone(),
                mime_type: Some(content.mime_type.to_string()),
                text: content.text,
                meta: None,
            }],
        })
    }
}

// ── Public API ─────────────────────────────────────────────────────────

/// Start the RMCP stdio MCP server.
pub async fn serve(server: LuftMcpServer) -> anyhow::Result<()> {
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_server() -> LuftMcpServer {
        use luft_core::{MockBackend, MockBehavior, TokenUsage};
        use std::time::Duration;
        let backend = MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!({"result": "ok"}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        );
        let luft = luft::Luft::builder()
            .backend(backend)
            .base_dir(tempfile::TempDir::new().unwrap().keep())
            .build()
            .unwrap();
        LuftMcpServer::new(luft)
    }

    fn simulate_handshake(server: &LuftMcpServer, name: &str) {
        let _ = server.client_name.set(name.to_string());
    }

    // ── Client identity ──────────────────────────────────────────────────

    #[test]
    fn client_name_is_none_before_handshake() {
        let server = make_server();
        assert_eq!(server.client_name(), None);
        assert!(!server.is_codex());
    }

    #[test]
    fn is_codex_matches_case_insensitive() {
        for name in ["codex", "CODEX", "Codex", "CoDeX"] {
            let server = make_server();
            simulate_handshake(&server, name);
            assert_eq!(server.client_name(), Some(name));
            assert!(
                server.is_codex(),
                "expected {name:?} to be detected as codex"
            );
        }
    }

    #[test]
    fn is_codex_rejects_non_codex_clients() {
        for name in ["claude-code", "claude", "opencode", ""] {
            let server = make_server();
            simulate_handshake(&server, name);
            assert_eq!(server.client_name(), Some(name));
            assert!(
                !server.is_codex(),
                "{name:?} must not be detected as codex"
            );
        }
    }

    #[test]
    fn client_name_keeps_first_value() {
        let server = make_server();
        simulate_handshake(&server, "codex");
        simulate_handshake(&server, "claude-code");
        assert_eq!(server.client_name(), Some("codex"));
        assert!(server.is_codex());
    }
}
