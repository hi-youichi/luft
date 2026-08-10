//! MCP Server subcommand: `luft mcp-structured-output`.
//!
//! Accepts a structured result from an agent via the `workflow_validate_schema`
//! MCP tool. The agent calls this tool with `{"result": <JSON value>}`; this
//! server returns a success message. Schema validation is performed by the
//! luft scheduler after the session completes, not here.
//!
//! Also hosts the full MCP server (`luft mcp serve`) which exposes workflow
//! authoring resources and execution tools via the [`luft_mcp`] crate.

use anyhow::Result;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    schemars,
    tool, tool_handler, tool_router,
    ServerHandler, ServiceExt,
};
use serde::Deserialize;
use serde_json::Value;

// ── `luft mcp serve` — full MCP server ───────────────────────────────

/// Subcommand group: `luft mcp <action>`.
#[derive(Debug, clap::Subcommand)]
pub enum McpSubcommand {
    /// Start the MCP server on stdio (JSON-RPC over stdin/stdout).
    Serve(McpServeArgs),
}

/// Arguments for `luft mcp serve`.
#[derive(Debug, clap::Args)]
pub struct McpServeArgs {
    /// Backend id to use for workflow execution (default: auto-detect).
    #[arg(long, help = "Backend id (mock, opencode, codex) or auto-detect")]
    pub backend: Option<String>,
}

/// Entry point for `luft mcp serve`.
pub async fn serve(args: McpServeArgs) -> Result<()> {
    let addr = luft_daemon::discover_or_autostart().await?;
    luft_mcp::proxy::run_proxy(&addr, args.backend.as_deref()).await?;
    Ok(())
}

// ── `luft mcp-structured-output` — result acceptor ──────────────────

#[derive(Debug, clap::Args)]
pub struct McpWorkflowValidateSchemaArgs;

pub async fn run(_args: McpWorkflowValidateSchemaArgs) -> Result<()> {
    let log_path = std::env::var("LUFT_MCP_LOG").unwrap_or_else(|_| {
        let dir = std::env::temp_dir();
        dir.join(format!("luft-mcp-{}.log", std::process::id()))
            .to_string_lossy()
            .into_owned()
    });
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug")),
        )
        .with_writer(log_file)
        .try_init();

    tracing::info!(log = %log_path, "MCP structured-output server starting");

    let mut server = ValidateSchemaServer {
        tool_router: ToolRouter::default(),
    };
    server.tool_router = ValidateSchemaServer::tool_router();
    let (reader, writer) = rmcp::transport::io::stdio();
    let service = server.serve((reader, writer)).await?;
    service.waiting().await?;
    Ok(())
}

// ── RMCP server ─────────────────────────────────────────────────────

#[derive(Debug)]
struct ValidateSchemaServer {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl ValidateSchemaServer {
    #[tool(description = "Submit your final structured result.\n\nPass your result as {\"result\": <your JSON value>}.")]
    fn workflow_validate_schema(
        Parameters(params): Parameters<WorkflowValidateSchemaInput>,
    ) -> Result<String, String> {
        tracing::debug!(result = %params.result, "workflow_validate_schema accepted");
        Ok("Result submitted.".to_string())
    }
}

#[tool_handler]
impl ServerHandler for ValidateSchemaServer {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WorkflowValidateSchemaInput {
    /// The structured result to submit.
    result: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn input_deserializes_result() {
        let params = json!({"result": {"x": 42}});
        let input: WorkflowValidateSchemaInput = serde_json::from_value(params).unwrap();
        assert_eq!(input.result, json!({"x": 42}));
    }

    #[test]
    fn input_deserializes_null_result() {
        let params = json!({"result": null});
        let input: WorkflowValidateSchemaInput = serde_json::from_value(params).unwrap();
        assert_eq!(input.result, Value::Null);
    }

    #[test]
    fn input_deserializes_string_result() {
        let params = json!({"result": "hello"});
        let input: WorkflowValidateSchemaInput = serde_json::from_value(params).unwrap();
        assert_eq!(input.result, "hello");
    }

    #[test]
    fn input_fails_on_missing_result() {
        let params = json!({"input": {}});
        let result: std::result::Result<WorkflowValidateSchemaInput, _> =
            serde_json::from_value(params);
        assert!(result.is_err());
    }
}
