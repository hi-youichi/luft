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

#[derive(Debug, clap::Args)]
pub struct McpStructuredOutputArgs {
    #[arg(long, help = "Path to JSON Schema file")]
    pub schema_file: PathBuf,
}

pub fn run(args: McpStructuredOutputArgs) -> Result<()> {
    let log_path = std::env::var("MAESTRO_MCP_LOG").unwrap_or_else(|_| {
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
                tracing::info!(
                    is_error = result
                        .get("isError")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    "MCP tools/call response"
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Seek, Write};
    use std::sync::Mutex;

    /// Serialises fd-redirection tests so parallel runs don't race on fd 0/1.
    #[cfg(unix)]
    static IO_LOCK: Mutex<()> = Mutex::new(());

    // --- Raw FFI helpers for fd redirection (macOS / Linux) -------------------

    #[cfg(unix)]
    mod ffi {
        #![allow(dead_code)]
        pub unsafe fn dup(fd: std::os::raw::c_int) -> std::os::raw::c_int {
            extern "C" {
                fn dup(fd: std::os::raw::c_int) -> std::os::raw::c_int;
            }
            dup(fd)
        }
        pub unsafe fn dup2(oldfd: std::os::raw::c_int, newfd: std::os::raw::c_int) {
            extern "C" {
                fn dup2(
                    oldfd: std::os::raw::c_int,
                    newfd: std::os::raw::c_int,
                ) -> std::os::raw::c_int;
            }
            dup2(oldfd, newfd);
        }
        pub unsafe fn close(fd: std::os::raw::c_int) {
            extern "C" {
                fn close(fd: std::os::raw::c_int) -> std::os::raw::c_int;
            }
            close(fd);
        }
    }

    // --- Helpers -------------------------------------------------------------

    /// Run `f` with stdin / stdout redirected from / to temporary files.
    /// Returns `(f()'s return value, lines written to stdout)`.
    /// **MUST** be called while holding `IO_LOCK`.
    #[cfg(unix)]
    fn with_redirected_io<R>(input: &str, f: impl FnOnce() -> R) -> (R, Vec<String>) {
        let mut in_file = tempfile::tempfile().expect("tempfile in");
        in_file.write_all(input.as_bytes()).unwrap();
        in_file.flush().unwrap();
        in_file.seek(std::io::SeekFrom::Start(0)).unwrap();

        let out_file = tempfile::tempfile().expect("tempfile out");
        let out_read = out_file.try_clone().expect("try_clone");

        use std::os::unix::io::IntoRawFd;
        let in_fd = in_file.into_raw_fd();
        let out_fd = out_file.into_raw_fd();
        let out_read_fd = out_read.into_raw_fd();

        unsafe {
            let saved_stdin = ffi::dup(0);
            let saved_stdout = ffi::dup(1);

            ffi::dup2(in_fd, 0);
            ffi::close(in_fd);
            ffi::dup2(out_fd, 1);
            ffi::close(out_fd);

            let result = f();

            ffi::dup2(saved_stdin, 0);
            ffi::close(saved_stdin);
            ffi::dup2(saved_stdout, 1);
            ffi::close(saved_stdout);

            // Read captured output
            use std::os::unix::io::FromRawFd;
            let mut out_file = std::fs::File::from_raw_fd(out_read_fd);
            out_file.seek(std::io::SeekFrom::Start(0)).unwrap();
            let reader = BufReader::new(out_file);
            let lines: Vec<String> = reader
                .lines()
                .map(|l| l.unwrap())
                .filter(|l| !l.is_empty())
                .collect();

            (result, lines)
        }
    }

    fn json_lines(lines: &[String]) -> Vec<Value> {
        lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[cfg(unix)]
    fn run_serve_mcp(input_lines: &[&str], schema: &Value) -> Vec<String> {
        let input = if input_lines.is_empty() {
            String::new()
        } else {
            input_lines.join("\n") + "\n"
        };
        let _lock = IO_LOCK.lock().unwrap();
        let (_, out_lines) = with_redirected_io(&input, || {
            let _ = serve_mcp(schema);
        });
        out_lines
    }

    // ------------------------------------------------------------------
    //  validate_against_schema
    // ------------------------------------------------------------------

    #[test]
    fn validate_valid_input() {
        let schema =
            serde_json::json!({"type": "object", "properties": {"x": {"type": "integer"}}});
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
            "expected ≤2 separators (≤3 errors), got {semicolons}"
        );
    }

    #[test]
    fn validate_schema_compile_error() {
        // `type` expects a string or array of strings, not an integer.
        let schema = serde_json::json!({"type": 123});
        let input = serde_json::json!("hello");
        let result = validate_against_schema(&input, &schema);
        assert!(result.is_err(), "expected compile error for invalid schema");
    }

    // ------------------------------------------------------------------
    //  handle_tool_call
    // ------------------------------------------------------------------

    #[test]
    fn tool_call_unknown_tool() {
        let schema = serde_json::json!({"type": "object"});
        let params = serde_json::json!({"name": "unknown_tool", "arguments": {}});
        let result = handle_tool_call(&Some(params), &schema);
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Unknown tool: unknown_tool"));
    }

    #[test]
    fn tool_call_no_params() {
        let schema = serde_json::json!({"type": "object"});
        let result = handle_tool_call(&None, &schema);
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Unknown tool: "));
    }

    #[test]
    fn tool_call_no_name_field() {
        let schema = serde_json::json!({"type": "object"});
        let params = serde_json::json!({"arguments": {}});
        let result = handle_tool_call(&Some(params), &schema);
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Unknown tool: "));
    }

    #[test]
    fn tool_call_valid_arguments() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"result": {"type": "string"}},
            "required": ["result"]
        });
        let params =
            serde_json::json!({"name": "structured_output", "arguments": {"result": "ok"}});
        let result = handle_tool_call(&Some(params), &schema);
        assert_eq!(result["isError"], false);
        assert_eq!(result["content"][0]["text"], "Result accepted.");
    }

    #[test]
    fn tool_call_invalid_arguments() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"result": {"type": "string"}},
            "required": ["result"]
        });
        let params = serde_json::json!({"name": "structured_output", "arguments": {"result": 42}});
        let result = handle_tool_call(&Some(params), &schema);
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Schema validation failed"));
    }

    #[test]
    fn tool_call_missing_arguments() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"result": {"type": "string"}},
            "required": ["result"]
        });
        let params = serde_json::json!({"name": "structured_output"});
        let result = handle_tool_call(&Some(params), &schema);
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Schema validation failed"));
    }

    #[test]
    fn tool_call_with_file_kind_summary_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "file": {"type": "string"},
                "kind": {"type": "string"},
                "summary": {"type": "string"}
            },
            "required": ["file", "kind", "summary"]
        });

        let params = serde_json::json!({
            "name": "structured_output",
            "arguments": {
                "file": "src/adapters/result_collector.rs",
                "kind": "rust",
                "summary": "collects agent results"
            }
        });
        let result = handle_tool_call(&Some(params), &schema);
        assert_eq!(result["isError"], false);
        assert_eq!(result["content"][0]["text"], "Result accepted.");

        let missing = serde_json::json!({
            "name": "structured_output",
            "arguments": {
                "file": "src/adapters/result_collector.rs",
                "kind": "rust"
            }
        });
        let result = handle_tool_call(&Some(missing), &schema);
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Schema validation failed"));
    }

    // ------------------------------------------------------------------
    //  write_response / write_error
    // ------------------------------------------------------------------

    #[test]
    fn write_response_numeric_id() {
        let mut buf = Vec::new();
        let id = serde_json::json!(42);
        let result = serde_json::json!({"ok": true});
        write_response(&mut buf, id, &result).unwrap();
        let resp: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 42);
        assert_eq!(resp["result"]["ok"], true);
    }

    #[test]
    fn write_response_string_id() {
        let mut buf = Vec::new();
        let id = serde_json::json!("req-1");
        let result = serde_json::json!({"ok": true});
        write_response(&mut buf, id, &result).unwrap();
        let resp: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(resp["id"], "req-1");
    }

    #[test]
    fn write_error_ok() {
        let mut buf = Vec::new();
        let id = serde_json::json!(1);
        write_error(&mut buf, id, -32601, "Method not found").unwrap();
        let resp: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["error"]["code"], -32601);
        assert_eq!(resp["error"]["message"], "Method not found");
    }

    // ------------------------------------------------------------------
    //  JsonRpcMessage deserialization
    // ------------------------------------------------------------------

    #[test]
    fn json_rpc_message_full() {
        let msg: JsonRpcMessage =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
                .unwrap();
        assert_eq!(msg.method.as_deref(), Some("initialize"));
        assert_eq!(msg.id, Some(serde_json::json!(1)));
        assert!(msg.params.is_some());
    }

    #[test]
    fn json_rpc_message_notification() {
        let msg: JsonRpcMessage =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .unwrap();
        assert_eq!(msg.method.as_deref(), Some("notifications/initialized"));
        assert!(msg.id.is_none());
        assert!(msg.params.is_none());
    }

    #[test]
    fn json_rpc_message_no_id_no_method() {
        let msg: JsonRpcMessage = serde_json::from_str(r#"{"jsonrpc":"2.0"}"#).unwrap();
        assert!(msg.method.is_none());
        assert!(msg.id.is_none());
        assert!(msg.params.is_none());
    }

    // ------------------------------------------------------------------
    //  serve_mcp — integration via fd redirection
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn serve_mcp_initialize() {
        let schema = serde_json::json!({"type": "object"});
        let lines = run_serve_mcp(
            &[r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#],
            &schema,
        );
        let json = json_lines(&lines);
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["id"], 1);
        assert_eq!(
            json[0]["result"]["serverInfo"]["name"],
            "maestro-structured-output"
        );
    }

    #[cfg(unix)]
    #[test]
    fn serve_mcp_notification_initialized() {
        let schema = serde_json::json!({"type": "object"});
        let lines = run_serve_mcp(
            &[r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#],
            &schema,
        );
        assert!(lines.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn serve_mcp_tools_list() {
        let schema = serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}});
        let lines = run_serve_mcp(
            &[r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#],
            &schema,
        );
        let json = json_lines(&lines);
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["id"], 2);
        assert_eq!(json[0]["result"]["tools"][0]["name"], "structured_output");
        assert_eq!(json[0]["result"]["tools"][0]["inputSchema"], schema);
    }

    #[cfg(unix)]
    #[test]
    fn serve_mcp_tools_call_valid() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"result": {"type": "string"}},
            "required": ["result"]
        });
        let lines = run_serve_mcp(
            &[
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"structured_output","arguments":{"result":"done"}}}"#,
            ],
            &schema,
        );
        let json = json_lines(&lines);
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["id"], 3);
        assert_eq!(json[0]["result"]["isError"], false);
        assert_eq!(json[0]["result"]["content"][0]["text"], "Result accepted.");
    }

    #[cfg(unix)]
    #[test]
    fn serve_mcp_tools_call_invalid() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"result": {"type": "string"}},
            "required": ["result"]
        });
        let lines = run_serve_mcp(
            &[
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"structured_output","arguments":{"result":42}}}"#,
            ],
            &schema,
        );
        let json = json_lines(&lines);
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["id"], 4);
        assert_eq!(json[0]["result"]["isError"], true);
        assert!(json[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Schema validation failed"));
    }

    #[cfg(unix)]
    #[test]
    fn serve_mcp_unknown_method_with_id() {
        let schema = serde_json::json!({"type": "object"});
        let lines = run_serve_mcp(
            &[r#"{"jsonrpc":"2.0","id":5,"method":"unknown/method"}"#],
            &schema,
        );
        let json = json_lines(&lines);
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["id"], 5);
        assert_eq!(json[0]["error"]["code"], -32601);
        assert_eq!(json[0]["error"]["message"], "Method not found");
    }

    #[cfg(unix)]
    #[test]
    fn serve_mcp_unknown_method_no_id() {
        let schema = serde_json::json!({"type": "object"});
        let lines = run_serve_mcp(&[r#"{"jsonrpc":"2.0","method":"unknown/method"}"#], &schema);
        assert!(lines.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn serve_mcp_empty_line_skipped() {
        let schema = serde_json::json!({"type": "object"});
        let lines = run_serve_mcp(
            &["", r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#],
            &schema,
        );
        let json = json_lines(&lines);
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["id"], 1);
    }

    #[cfg(unix)]
    #[test]
    fn serve_mcp_malformed_json_skipped() {
        let schema = serde_json::json!({"type": "object"});
        let lines = run_serve_mcp(
            &[
                "not valid json",
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            ],
            &schema,
        );
        let json = json_lines(&lines);
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["id"], 1);
    }

    // ------------------------------------------------------------------
    //  run — integration
    // ------------------------------------------------------------------

    /// Helper: call `run()` with a real schema file and redirected I/O.
    #[cfg(unix)]
    fn run_with_input(input: &str, schema_body: &Value) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("schema.json");
        std::fs::write(
            &schema_path,
            serde_json::to_string_pretty(schema_body).unwrap(),
        )
        .unwrap();

        let _lock = IO_LOCK.lock().unwrap();
        let (_, out_lines) = with_redirected_io(input, || {
            let args = McpStructuredOutputArgs {
                schema_file: schema_path,
            };
            let _ = run(args);
        });
        out_lines
    }

    #[test]
    fn run_missing_schema_file() {
        // The default log path creates a file under temp_dir, which should succeed.
        // The schema file does NOT exist → run() returns Err.
        let args = McpStructuredOutputArgs {
            schema_file: "/tmp/nonexistent_schema_maestro_test.json".into(),
        };
        let result = run(args);
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn run_with_schema_file_and_initialize() {
        let schema = serde_json::json!({"type": "object"});
        let lines = run_with_input(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#, &schema);
        let json = json_lines(&lines);
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["id"], 1);
        assert_eq!(
            json[0]["result"]["serverInfo"]["name"],
            "maestro-structured-output"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_with_env_log_var() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("schema.json");
        let schema = serde_json::json!({"type": "object"});
        std::fs::write(&schema_path, serde_json::to_string_pretty(&schema).unwrap()).unwrap();
        let log_path = dir.path().join("custom-mcp.log");

        std::env::set_var("MAESTRO_MCP_LOG", log_path.to_str().unwrap());

        let _lock = IO_LOCK.lock().unwrap();
        let (_, out_lines) =
            with_redirected_io(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#, || {
                let args = McpStructuredOutputArgs {
                    schema_file: schema_path.clone(),
                };
                let _ = run(args);
            });
        let json = json_lines(&out_lines);
        assert_eq!(json.len(), 1);
        assert!(log_path.exists(), "custom log file should exist");

        std::env::remove_var("MAESTRO_MCP_LOG");
    }

    // ------------------------------------------------------------------
    //  McpStructuredOutputArgs
    // ------------------------------------------------------------------

    #[test]
    fn mcp_args_has_schema_file() {
        let args = McpStructuredOutputArgs {
            schema_file: "test.json".into(),
        };
        assert_eq!(args.schema_file.display().to_string(), "test.json");
    }
}
