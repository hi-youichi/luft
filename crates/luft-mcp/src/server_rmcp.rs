//! RMCP server — uses `rmcp` SDK instead of hand-rolled JSON-RPC.
//!
//! Exposes the same 6 tools and 3 resource URIs as the legacy `server.rs`.
//! Business logic is delegated to `crate::tools` and `crate::resources`.

use luft::Luft;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::Parameters,
    },
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// RMCP-based MCP server.
#[derive(Clone)]
pub struct LuftMcpServer {
    luft: Arc<Luft>,
    search_dirs: Vec<PathBuf>,
    tool_router: ToolRouter<Self>,
}

impl LuftMcpServer {
    pub fn new(luft: Luft) -> Self {
        let mut s = Self {
            luft: Arc::new(luft),
            search_dirs: vec![PathBuf::from("examples"), PathBuf::from("workflows")],
            tool_router: ToolRouter::default(),
        };
        s.tool_router = Self::tool_router();
        s
    }

    pub fn search_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.search_dirs = dirs;
        self
    }
}

// ── Arguments ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct RunIdParams {
    run_id: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct ListRunsParams {
    limit: Option<u64>,
    cursor: Option<String>,
    status_filter: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct GetRunEventsParams {
    run_id: String,
    since_event_id: Option<String>,
    offset: Option<u64>,
    events_limit: Option<u64>,
    types: Option<Vec<String>>,
    agent_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct ExecuteWorkflowParams {
    script: Option<String>,
    path: Option<String>,
    resume_from_id: Option<String>,
    args: Option<serde_json::Value>,
    concurrency: Option<u64>,
}

// ── Tools ───────────────────────────────────────────────────────────────

fn legacy_json_to_text(v: serde_json::Value) -> Result<String, String> {
    let is_error = v
        .get("isError")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let text = v
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if is_error {
        Err(text.to_string())
    } else {
        Ok(text.to_string())
    }
}

#[tool_router]
impl LuftMcpServer {
    #[tool(description = "Execute a Luft workflow, or resume a prior checkpointed run. Exactly one of `script`, `path`, `resume_from_id` is required. Returns immediately with a run_id — use get_run_status to poll progress.")]
    async fn execute_workflow(
        &self,
        Parameters(params): Parameters<ExecuteWorkflowParams>,
    ) -> Result<String, String> {
        let args = serde_json::to_value(&params).unwrap_or_default();
        let result = crate::tools::execute_workflow(&self.luft, &args).await;
        legacy_json_to_text(result)
    }

    #[tool(description = "List available .lua workflow files from workflows/ and examples/ directories")]
    fn list_files(&self) -> Result<String, String> {
        let result = crate::tools::list_files_tool(&self.search_dirs);
        legacy_json_to_text(result)
    }

    #[tool(description = "List past workflow runs, paginated and optionally filtered by terminal status")]
    fn list_runs(&self, Parameters(params): Parameters<ListRunsParams>) -> Result<String, String> {
        let args = serde_json::to_value(&params).unwrap_or_default();
        let result = crate::tools::list_runs_tool(&self.luft, &args);
        legacy_json_to_text(result)
    }

    #[tool(description = "Get the current rich status of a workflow run, including per-phase and per-agent detail")]
    fn get_run_status(&self, Parameters(params): Parameters<RunIdParams>) -> Result<String, String> {
        let args = serde_json::to_value(&params).unwrap_or_default();
        let result = crate::tools::get_run_status_tool(&self.luft, &args);
        legacy_json_to_text(result)
    }

    #[tool(description = "Get paginated/filtered events for a workflow run")]
    fn get_run_events(&self, Parameters(params): Parameters<GetRunEventsParams>) -> Result<String, String> {
        let args = serde_json::to_value(&params).unwrap_or_default();
        let result = crate::tools::get_run_events_tool(&self.luft, &args);
        legacy_json_to_text(result)
    }

    #[tool(description = "Cancel an in-flight workflow run")]
    fn cancel_run(&self, Parameters(params): Parameters<RunIdParams>) -> Result<String, String> {
        let args = serde_json::to_value(&params).unwrap_or_default();
        let result = crate::tools::cancel_run_tool(&self.luft, &args);
        legacy_json_to_text(result)
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
            McpError::resource_not_found(
                "unknown_uri",
                Some(serde_json::json!({ "uri": uri })),
            )
        })?;

        let content = crate::resources::read_resource(&parsed, &self.search_dirs).map_err(|e| {
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

/// Start the RMCP stdio MCP server (replaces `McpServer::serve_stdio`).
pub async fn serve(server: LuftMcpServer) -> anyhow::Result<()> {
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
