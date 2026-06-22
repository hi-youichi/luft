//! MCP Server subcommand: `maestro mcp-structured-output --schema-file <path>`.
//!
//! Speaks minimal MCP (JSON-RPC over stdio) with a single `structured_output`
//! tool whose `inputSchema` is the workflow-provided JSON Schema.
//! opencode spawns this as a subprocess via `NewSessionRequest.mcp_servers`.

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct McpStructuredOutputArgs {
    #[arg(long, help = "Path to JSON Schema file")]
    schema_file: PathBuf,
}

pub fn run(args: McpStructuredOutputArgs) -> Result<()> {
    let log_path = std::env::var("MAESTRO_MCP_LOG")
        .unwrap_or_else(|_| {
            let dir = std::env::temp_dir();
            dir.join(format!("maestro-mcp-{}.log", std::process::id()))
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

    tracing::info!(schema_file = %args.schema_file.display(), log = %log_path, "MCP structured-output server starting");
    let schema: Value = serde_json::from_str(&std::fs::read_to_string(&args.schema_file)?)?;
    serve_mcp(&schema)
}

fn serve_mcp(schema: &Value) -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        tracing::debug!(line = %line, "MCP recv");

        let msg: JsonRpcMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, line = %line, "MCP parse error");
                continue;
            }
        };

        let method = msg.method.as_deref();
        let id = msg.id.clone();

        match (method, id) {
            (Some("initialize"), Some(id)) => {
                let result = serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "maestro-structured-output", "version": "0.1.0" }
                });
                write_response(&mut stdout, id, &result)?;
            }
            (Some("notifications/initialized"), _) => {}
            (Some("tools/list"), Some(id)) => {
                let result = serde_json::json!({
                    "tools": [{
                        "name": "structured_output",
                        "description": format!(
                            "Call this tool to submit your final result.\n\
                             The result MUST be a JSON object matching this schema:\n\n\
                             {schema}\n\n\
                             Do NOT return the result as a text message. \
                             You MUST call this tool.",
                            schema = serde_json::to_string_pretty(schema).unwrap_or_default()
                        ),
                        "inputSchema": schema,
                    }]
                });
                write_response(&mut stdout, id, &result)?;
            }
            (Some("tools/call"), Some(id)) => {
                tracing::info!(params = ?msg.params, "MCP tools/call");
                let result = handle_tool_call(&msg.params, schema);
                tracing::info!(is_error = result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false), "MCP tools/call response");
                write_response(&mut stdout, id, &result)?;
            }
            (_, Some(id)) => {
                write_error(&mut stdout, id, -32601, "Method not found")?;
            }
            _ => {}
        }
    }

    Ok(())
}

fn handle_tool_call(params: &Option<Value>, schema: &Value) -> Value {
    let name = params
        .as_ref()
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");

    if name != "structured_output" {
        return serde_json::json!({
            "content": [{ "type": "text", "text": format!("Unknown tool: {name}") }],
            "isError": true
        });
    }

    let input = params
        .as_ref()
        .and_then(|p| p.get("arguments"))
        .cloned()
        .unwrap_or(Value::Null);

    match validate_against_schema(&input, schema) {
        Ok(()) => serde_json::json!({
            "content": [{ "type": "text", "text": "Result accepted." }],
            "isError": false
        }),
        Err(msg) => serde_json::json!({
            "content": [{ "type": "text", "text": format!(
                "Schema validation failed: {msg}\nPlease correct your output and call this tool again."
            )}],
            "isError": true
        }),
    }
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

fn write_response(stdout: &mut impl Write, id: Value, result: &Value) -> Result<()> {
    let resp = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
    tracing::debug!(response = %resp, "MCP send");
    writeln!(stdout, "{}", resp)?;
    stdout.flush()?;
    Ok(())
}

fn write_error(stdout: &mut impl Write, id: Value, code: i32, message: &str) -> Result<()> {
    let resp = serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } });
    writeln!(stdout, "{}", resp)?;
    stdout.flush()?;
    Ok(())
}

#[derive(Deserialize)]
struct JsonRpcMessage {
    method: Option<String>,
    id: Option<Value>,
    params: Option<Value>,
}
