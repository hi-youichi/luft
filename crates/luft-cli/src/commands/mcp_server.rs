//! MCP Server subcommand: `luft mcp-structured-output`.
//!
//! Validates a JSON object against a JSON Schema (Draft 7) via MCP tool.
//! opencode spawns this as a subprocess via `NewSessionRequest.mcp_servers`.
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
    let addr = luft_daemon::discover_or_autostart(args.backend).await?;
    luft_mcp::proxy::run_proxy(&addr).await?;
    Ok(())
}

// ── `luft mcp-structured-output` — schema validator ─────────────────

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
    #[tool(description = "Validate a JSON object against a JSON Schema (Draft 7).\n\nBoth `input` and `schema` are required.")]
    fn workflow_validate_schema(
        Parameters(params): Parameters<WorkflowValidateSchemaInput>,
    ) -> Result<String, String> {
        validate_against_schema(&params.input, &params.schema)
            .map(|_| "Result accepted.".to_string())
            .map_err(|e| format!("Schema validation failed: {e}\nPlease correct your output and call this tool again."))
    }
}

#[tool_handler]
impl ServerHandler for ValidateSchemaServer {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WorkflowValidateSchemaInput {
    /// The data to validate.
    input: Value,
    /// JSON Schema (Draft 7) to validate against.
    schema: Value,
}

fn validate_against_schema(input: &Value, schema: &Value) -> std::result::Result<(), String> {
    let validator = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(schema)
        .map_err(|e| format!("schema compile error: {e}"))?;

    let result = validator.validate(input);
    match result {
        Ok(()) => Ok(()),
        Err(errors) => {
            let details: Vec<String> = errors
                .take(3)
                .map(|e| format!("instance {}: {}", e.instance_path, e))
                .collect();
            Err(details.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── validate_against_schema unit tests ───────────────────────────────

    #[test]
    fn validate_valid_input() {
        let schema = serde_json::json!({"type": "object", "properties": {"x": {"type": "integer"}}});
        let input = serde_json::json!({"x": 42});
        assert!(validate_against_schema(&input, &schema).is_ok());
    }

    #[test]
    fn validate_invalid_type() {
        let schema = serde_json::json!(
            {"type": "object", "properties": {"x": {"type": "integer"}}, "required": ["x"]}
        );
        let input = serde_json::json!({"x": "not-a-number"});
        let err = validate_against_schema(&input, &schema).unwrap_err();
        assert!(err.contains("instance"), "got: {err}");
    }

    #[test]
    fn validate_missing_required() {
        let schema = serde_json::json!(
            {"type": "object", "properties": {"x": {"type": "integer"}}, "required": ["x"]}
        );
        let input = serde_json::json!({});
        let err = validate_against_schema(&input, &schema).unwrap_err();
        assert!(err.contains("instance"), "got: {err}");
    }

    #[test]
    fn validate_multiple_errors_capped_at_three() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer", "minimum": 0, "maximum": 10},
                "b": {"type": "string"},
                "c": {"type": "array", "minItems": 1, "items": {"type": "string"}}
            },
            "required": ["a", "b", "c"]
        });
        let input = serde_json::json!({"a": -1, "b": 42, "c": "not-an-array"});
        let err = validate_against_schema(&input, &schema).unwrap_err();
        assert!(err.contains("instance"), "got: {err}");
        let semicolons = err.matches(';').count();
        assert!(
            semicolons <= 2,
            "expected \u{2264}2 separators (\u{2264}3 errors), got {semicolons}"
        );
    }

    #[test]
    fn validate_schema_compile_error() {
        let schema = serde_json::json!({"type": 123});
        let input = serde_json::json!("hello");
        let err = validate_against_schema(&input, &schema).unwrap_err();
        assert!(
            err.starts_with("schema compile error:"),
            "expected 'schema compile error:' prefix, got: {err}"
        );
    }

    #[test]
    fn validate_against_custom_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "file": {"type": "string"},
                "kind": {"type": "string"},
                "summary": {"type": "string"}
            },
            "required": ["file", "kind", "summary"]
        });

        let valid = serde_json::json!({
            "file": "src/main.rs",
            "kind": "rust",
            "summary": "entry point"
        });
        assert!(validate_against_schema(&valid, &schema).is_ok());

        let invalid = serde_json::json!({
            "file": "src/main.rs",
            "kind": "rust"
        });
        assert!(validate_against_schema(&invalid, &schema).is_err());
    }

    // ── WorkflowValidateSchemaInput deserialization ─────────────────────

    #[test]
    fn input_deserializes_from_full_params() {
        let params = json!({
            "input": {"x": 42},
            "schema": {"type": "object", "properties": {"x": {"type": "integer"}}}
        });
        let input: WorkflowValidateSchemaInput = serde_json::from_value(params).unwrap();
        assert_eq!(input.input, json!({"x": 42}));
        assert_eq!(
            input.schema,
            json!({"type": "object", "properties": {"x": {"type": "integer"}}})
        );
    }

    #[test]
    fn input_deserializes_input_as_null() {
        let params = json!({
            "input": null,
            "schema": {"type": "null"}
        });
        let input: WorkflowValidateSchemaInput = serde_json::from_value(params).unwrap();
        assert_eq!(input.input, Value::Null);
    }

    #[test]
    fn input_deserializes_schema_as_any_json() {
        let params = json!({
            "input": "hello",
            "schema": {"type": "string"}
        });
        let input: WorkflowValidateSchemaInput = serde_json::from_value(params).unwrap();
        assert_eq!(input.input, "hello");
    }

    #[test]
    fn input_fails_on_missing_fields() {
        let params = json!({"input": {}});
        let result: Result<WorkflowValidateSchemaInput, _> = serde_json::from_value(params);
        assert!(result.is_err(), "expected deserialization error for missing schema");
    }

    // ── validate_against_schema edge cases ──────────────────────────────

    #[test]
    fn validate_edge_empty_string() {
        let schema = json!({"type": "string"});
        assert!(validate_against_schema(&json!(""), &schema).is_ok());
    }

    #[test]
    fn validate_edge_null_value() {
        let schema = json!({"type": "null"});
        assert!(validate_against_schema(&Value::Null, &schema).is_ok());
    }

    #[test]
    fn validate_edge_nested_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "data": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "name": {"type": "string"}
                    },
                    "required": ["id"]
                }
            },
            "required": ["data"]
        });
        let valid = json!({"data": {"id": 1, "name": "test"}});
        let invalid = json!({"data": {"name": "no-id"}});
        assert!(validate_against_schema(&valid, &schema).is_ok());
        assert!(validate_against_schema(&invalid, &schema).is_err());
    }

    #[test]
    fn validate_edge_array_items() {
        let schema = json!({
            "type": "array",
            "items": {"type": "integer"},
            "minItems": 1
        });
        let valid = json!([1, 2, 3]);
        let invalid = json!(["a", "b"]);
        let empty = json!([]);
        assert!(validate_against_schema(&valid, &schema).is_ok());
        assert!(validate_against_schema(&invalid, &schema).is_err());
        assert!(validate_against_schema(&empty, &schema).is_err());
    }

    #[test]
    fn validate_edge_enum_values() {
        let schema = json!({
            "type": "string",
            "enum": ["red", "green", "blue"]
        });
        assert!(validate_against_schema(&json!("red"), &schema).is_ok());
        assert!(validate_against_schema(&json!("yellow"), &schema).is_err());
    }

    #[test]
    fn validate_edge_number_bounds() {
        let schema = json!({
            "type": "number",
            "minimum": 0,
            "maximum": 100
        });
        assert!(validate_against_schema(&json!(50), &schema).is_ok());
        assert!(validate_against_schema(&json!(-1), &schema).is_err());
        assert!(validate_against_schema(&json!(101), &schema).is_err());
    }
}