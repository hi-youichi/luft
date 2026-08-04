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
use luft_service::{WorkflowService, WorkflowServiceImpl};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
};
use std::path::PathBuf;
use std::sync::Arc;

// ── LuftMcpServer ──────────────────────────────────────────────────────

/// RMCP-based MCP server — a thin facade over `WorkflowServiceImpl`.
#[derive(Clone)]
pub struct LuftMcpServer {
    pub service: Arc<WorkflowServiceImpl>,
    tool_router: ToolRouter<Self>,
    pub default_backend: Option<String>,
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
            default_backend: None,
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

    /// Create a clone with optional per-connection backend.
    /// Used by the daemon accept loop for each WS connection.
    pub fn with_fresh_client_name_and_backend(&self, default_backend: Option<String>) -> Self {
        Self {
            service: Arc::clone(&self.service),
            tool_router: self.tool_router.clone(),
            default_backend,
        }
    }
}

// ── Tools ──────────────────────────────────────────────────────────────

#[tool_router]
impl LuftMcpServer {
    #[tool(
        description = "Execute a Luft workflow, or resume a prior checkpointed run. Exactly one of `script`, `path`, `resume_from_id` is required. Returns immediately with a run_id — use workflow_status to poll progress. Pass `backend` to override (e.g. 'codex', 'opencode')."
    )]
    async fn workflow_execute(
        &self,
        Parameters(mut req): Parameters<ExecuteWorkflowRequest>,
    ) -> Result<String, String> {
        if req.backend.is_none() {
            if let Some(ref b) = self.default_backend {
                req.backend = Some(b.clone());
            }
        }
        let (resp, handle) = self
            .service
            .start_workflow(req)
            .await
            .map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            let _ = handle.join().await;
        });
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }

    #[tool(
        description = "List available .lua workflow files from workflows/ and examples/ directories"
    )]
    fn workflow_list_files(&self) -> Result<String, String> {
        let resp = self.service.list_files().map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }

    #[tool(
        description = "List past workflow runs, paginated and optionally filtered by terminal status"
    )]
    fn workflow_list_runs(
        &self,
        Parameters(req): Parameters<ListRunsRequest>,
    ) -> Result<String, String> {
        let resp = self.service.list_runs(req).map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Get the current rich status of a workflow run, including per-phase and per-agent detail"
    )]
    fn workflow_status(
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
    fn workflow_events(
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
    fn workflow_cancel(
        &self,
        Parameters(req): Parameters<CancelRunRequest>,
    ) -> Result<String, String> {
        let resp = self.service.cancel_run(req).map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Submit a structured result. Call this tool with a JSON object to deliver your final output. The result field accepts any JSON value."
    )]
    fn workflow_validate_schema(
        &self,
        Parameters(req): Parameters<WorkflowValidateSchemaRequest>,
    ) -> Result<String, String> {
        serde_json::to_string(&req).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct WorkflowValidateSchemaRequest {
    pub result: serde_json::Value,
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
                    description: Some("Complete Lua DSL syntax for writing Luft workflows".into()),
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
            McpError::resource_not_found("unknown_uri", Some(serde_json::json!({ "uri": uri })))
        })?;

        let content = crate::resources::read_resource(&parsed, self.service.search_dirs())
            .map_err(|e| {
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

    // ── Per-connection backend isolation ─────────────────────────────────

    #[test]
    fn with_fresh_client_name_and_backend_passes_default_backend() {
        let server = make_server();
        assert_eq!(server.default_backend, None);

        let server2 = server.with_fresh_client_name_and_backend(Some("codex".into()));
        assert_eq!(server2.default_backend, Some("codex".into()));
        assert_eq!(server.default_backend, None, "original should be unchanged");
    }

    #[test]
    fn with_fresh_client_name_and_backend_shares_service() {
        let server = make_server();
        let server2 = server.with_fresh_client_name_and_backend(Some("opencode".into()));
        assert!(
            Arc::ptr_eq(&server.service, &server2.service),
            "service Arc should be shared"
        );
    }

    #[test]
    fn default_backend_is_none_by_default() {
        let server = make_server();
        assert_eq!(server.default_backend, None);
    }
}
