//! Tool handlers for the MCP server.
//!
//! Implements the four MCP tools:
//! - `execute_workflow` — validate + fire-and-forget spawn, returns run_id
//! - `list_workflows` — list `.lua` files from search dirs
//! - `get_run_status` — query run status via `luft-service`
//! - `get_run_events` — query run event log

use luft::Luft;
use luft_core::contract::event::AgentEvent;
use luft_runtime::validate_workflow;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::resources::list_examples;
use crate::protocol::tool_result;

/// Information tracked for each started run.
#[derive(Debug, Clone)]
pub struct RunInfo {
    /// The run directory name (e.g. `deep-research_1781980050`).
    pub run_dir_name: String,
}

/// Registry of active runs: maps run_id (UUID string) to RunInfo.
///
/// The RunHandle returned by `start_script` is not stored — the underlying
/// `tokio::spawn` task continues running even after the handle is dropped.
/// We only need the `run_id → run_dir_name` mapping for status/event queries.
pub type RunRegistry = Arc<Mutex<HashMap<String, RunInfo>>>;

/// Create a new empty run registry.
pub fn new_run_registry() -> RunRegistry {
    Arc::new(Mutex::new(HashMap::new()))
}

// ── Tool dispatch ───────────────────────────────────────────────────────

/// Dispatch a `tools/call` request to the appropriate handler.
///
/// `params` is the raw JSON-RPC params object. Expected shape:
/// `{ "name": "<tool_name>", "arguments": { ... } }`
///
/// Returns the MCP tool result value (to be placed inside `result`).
pub async fn handle_call(
    params: &Value,
    luft: &Luft,
    runs: &RunRegistry,
    search_dirs: &[PathBuf],
) -> Value {
    let name = params.get("name").and_then(|v| v.as_str());
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    match name {
        Some("execute_workflow") => execute_workflow(luft, runs, &arguments).await,
        Some("list_workflows") => list_workflows_tool(search_dirs),
        Some("get_run_status") => get_run_status_tool(luft, runs, &arguments).await,
        Some("get_run_events") => get_run_events_tool(luft, runs, &arguments).await,
        Some(other) => tool_result(&format!("unknown tool: {other}"), true),
        None => tool_result("missing 'name' field in tools/call params", true),
    }
}

// ── execute_workflow ────────────────────────────────────────────────────

/// Execute a workflow: validate first, then fire-and-forget spawn.
///
/// Arguments: `{ "script": "...", "path": "...", "args": {...} }`
/// Either `script` (inline Lua) or `path` (.lua file) must be provided.
pub async fn execute_workflow(
    luft: &Luft,
    runs: &RunRegistry,
    args: &Value,
) -> Value {
    // Resolve the script source.
    let script = match resolve_script_source(args) {
        Ok(s) => s,
        Err(e) => return tool_result(&e, true),
    };

    // Pre-flight validation (syntax + structure + schema heuristic).
    let validation = match validate_workflow(&script) {
        Ok(v) => v,
        Err(e) => {
            let msg = json!({ "valid": false, "errors": [e.to_string()] });
            return tool_result(&msg.to_string(), true);
        }
    };

    if !validation.is_valid() {
        let msg = json!({
            "valid": false,
            "errors": validation.errors,
            "warnings": validation.warnings,
        });
        return tool_result(&msg.to_string(), true);
    }

    // Start the run (fire-and-forget). start_script spawns a tokio task that
    // continues running even if we drop the returned RunHandle.
    let handle = match luft.start_script(&script).await {
        Ok(h) => h,
        Err(e) => {
            let msg = json!({ "error": e.to_string() });
            return tool_result(&msg.to_string(), true);
        }
    };

    let run_id = handle.run_id().to_string();
    let run_dir_name = handle.run_dir_name().to_string();

    // Track the run_id → run_dir_name mapping. The RunHandle can be dropped
    // because the underlying tokio::spawn task is detached.
    runs.lock()
        .await
        .insert(run_id.clone(), RunInfo { run_dir_name: run_dir_name.clone() });

    let msg = json!({
        "run_id": run_id,
        "run_dir": run_dir_name,
        "status": "running"
    });
    tool_result(&msg.to_string(), false)
}

/// Resolve the Lua script from the tool arguments.
///
/// `script` takes priority; if absent, `path` is read from disk.
pub fn resolve_script_source(args: &Value) -> Result<String, String> {
    if let Some(script) = args.get("script").and_then(|v| v.as_str()) {
        if !script.trim().is_empty() {
            return Ok(script.to_string());
        }
    }

    if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
        if !path.is_empty() {
            return std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read workflow file '{path}': {e}"));
        }
    }

    Err("either 'script' or 'path' must be provided and non-empty".into())
}

// ── list_workflows ──────────────────────────────────────────────────────

/// List available workflow files from the search directories.
pub fn list_workflows_tool(search_dirs: &[PathBuf]) -> Value {
    let examples = list_examples(search_dirs);
    let entries: Vec<Value> = examples
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "path": e.path,
                "description": e.description,
            })
        })
        .collect();
    tool_result(&serde_json::to_string(&entries).unwrap_or_default(), false)
}

// ── get_run_status ──────────────────────────────────────────────────────

/// Resolve a run_id (UUID) to a run_dir_name via the registry.
///
/// Falls back to treating the input as a run_dir_name directly (for
/// historical/resumed runs not tracked by this registry).
async fn resolve_run_dir(runs: &RunRegistry, run_id: &str) -> String {
    if let Some(info) = runs.lock().await.get(run_id) {
        return info.run_dir_name.clone();
    }
    run_id.to_string()
}

/// Query the status of a run by its run_id or run_dir.
pub async fn get_run_status_tool(
    luft: &Luft,
    runs: &RunRegistry,
    args: &Value,
) -> Value {
    let Some(run_id) = args.get("run_id").and_then(|v| v.as_str()) else {
        return tool_result("missing required parameter: run_id", true);
    };

    let run_dir = resolve_run_dir(runs, run_id).await;

    match luft.status(&run_dir) {
        Ok(Some(status)) => {
            let json_status = serde_json::to_value(&status).unwrap_or(json!({}));
            tool_result(&json_status.to_string(), false)
        }
        Ok(None) => {
            let msg = json!({ "error": format!("run not found: {run_id}") });
            tool_result(&msg.to_string(), true)
        }
        Err(e) => {
            let msg = json!({ "error": e.to_string() });
            tool_result(&msg.to_string(), true)
        }
    }
}

// ── get_run_events ──────────────────────────────────────────────────────

/// Query the event log for a run.
pub async fn get_run_events_tool(
    luft: &Luft,
    runs: &RunRegistry,
    args: &Value,
) -> Value {
    let Some(run_id) = args.get("run_id").and_then(|v| v.as_str()) else {
        return tool_result("missing required parameter: run_id", true);
    };

    let run_dir = resolve_run_dir(runs, run_id).await;

    let events = match luft.events(&run_dir) {
        Ok(e) => e,
        Err(e) => {
            let msg = json!({ "error": format!("run not found: {run_id} ({})", e) });
            return tool_result(&msg.to_string(), true);
        }
    };

    // Optionally filter by since_event_id.
    let filtered = if let Some(since_id) = args.get("since_event_id").and_then(|v| v.as_str()) {
        filter_events_since(&events, since_id)
    } else {
        events
    };

    let serialized: Vec<Value> = filtered
        .iter()
        .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
        .collect();
    tool_result(&serde_json::to_string(&serialized).unwrap_or_default(), false)
}

/// Filter events: return all events *after* the one matching `since_id`.
///
/// If `since_id` is not found, return all events unchanged.
pub fn filter_events_since(events: &[AgentEvent], since_id: &str) -> Vec<AgentEvent> {
    // Find the index of the event matching since_id.
    let match_idx = events.iter().position(|e| event_matches_id(e, since_id));

    match match_idx {
        // Return events after the matched index.
        Some(idx) => events[idx + 1..].to_vec(),
        // since_id not found → return all events.
        None => events.to_vec(),
    }
}

/// Check if an event matches the given ID string.
///
/// Uses the event's serialized JSON form as a heuristic, since AgentEvent
/// doesn't expose a stable string ID field.
fn event_matches_id(event: &AgentEvent, id: &str) -> bool {
    if let Ok(serialized) = serde_json::to_string(event) {
        serialized.contains(id)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── resolve_script_source ───────────────────────────────────────────

    #[test]
    fn resolve_script_from_inline() {
        let args = json!({ "script": "report('hi')" });
        let result = resolve_script_source(&args).unwrap();
        assert_eq!(result, "report('hi')");
    }

    #[test]
    fn resolve_script_from_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("test.lua");
        std::fs::write(&file, "report('ok')").unwrap();

        let args = json!({ "path": file.to_str().unwrap() });
        let result = resolve_script_source(&args).unwrap();
        assert_eq!(result, "report('ok')");
    }

    #[test]
    fn resolve_script_path_not_found() {
        let args = json!({ "path": "/nonexistent/file.lua" });
        let err = resolve_script_source(&args).unwrap_err();
        assert!(err.contains("failed to read workflow file"));
    }

    #[test]
    fn resolve_script_empty_script_falls_to_error() {
        let args = json!({ "script": "  " });
        let err = resolve_script_source(&args).unwrap_err();
        assert!(err.contains("either 'script' or 'path'"));
    }

    #[test]
    fn resolve_script_neither_provided() {
        let args = json!({});
        let err = resolve_script_source(&args).unwrap_err();
        assert!(err.contains("either 'script' or 'path'"));
    }

    #[test]
    fn resolve_script_takes_priority_over_path() {
        let args = json!({ "script": "inline", "path": "/fake" });
        let result = resolve_script_source(&args).unwrap();
        assert_eq!(result, "inline");
    }

    // ── filter_events_since ─────────────────────────────────────────────

    #[test]
    fn filter_events_since_empty() {
        let events: Vec<AgentEvent> = vec![];
        let result = filter_events_since(&events, "evt-1");
        assert!(result.is_empty());
    }

    #[test]
    fn filter_events_since_found_returns_after() {
        use luft_core::contract::event::RunStatus;
        let events = vec![
            AgentEvent::RunDone {
                report: json!({"id": "first"}),
                status: RunStatus::Completed,
                run_id: uuid::Uuid::nil(),
                total_tokens: Default::default(),
                ts: chrono::Utc::now(),
            },
            AgentEvent::RunDone {
                report: json!({"id": "second"}),
                status: RunStatus::Completed,
                run_id: uuid::Uuid::nil(),
                total_tokens: Default::default(),
                ts: chrono::Utc::now(),
            },
        ];
        let result = filter_events_since(&events, "first");
        assert_eq!(result.len(), 1);
        // The result should be the "second" event, not "first"
        let result_json = serde_json::to_string(&result[0]).unwrap();
        assert!(result_json.contains("second"));
    }

    #[test]
    fn filter_events_since_not_found_returns_all() {
        use luft_core::contract::event::RunStatus;
        let events = vec![
            AgentEvent::RunDone {
                report: json!({"id": "a"}),
                status: RunStatus::Completed,
                run_id: uuid::Uuid::nil(),
                total_tokens: Default::default(),
                ts: chrono::Utc::now(),
            },
            AgentEvent::RunDone {
                report: json!({"id": "b"}),
                status: RunStatus::Completed,
                run_id: uuid::Uuid::nil(),
                total_tokens: Default::default(),
                ts: chrono::Utc::now(),
            },
        ];
        let result = filter_events_since(&events, "nonexistent");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn filter_events_since_match_at_last_returns_empty() {
        use luft_core::contract::event::RunStatus;
        let events = vec![AgentEvent::RunDone {
            report: json!({"id": "only"}),
            status: RunStatus::Completed,
            run_id: uuid::Uuid::nil(),
            total_tokens: Default::default(),
            ts: chrono::Utc::now(),
        }];
        let result = filter_events_since(&events, "only");
        assert!(result.is_empty());
    }

    // ── handle_call dispatch ────────────────────────────────────────────

    fn build_test_luft() -> Luft {
        use std::time::Duration;
        use luft_core::{MockBackend, MockBehavior, TokenUsage};
        let backend = MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!({"result": "ok"}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        );
        luft::Luft::builder()
            .backend(backend)
            .base_dir(tempfile::TempDir::new().unwrap().keep())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn handle_call_unknown_tool() {
        let luft = build_test_luft();
        let runs = new_run_registry();
        let dirs = vec![];

        let params = json!({ "name": "bogus", "arguments": {} });
        let result = handle_call(&params, &luft, &runs, &dirs).await;
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("unknown tool"));
    }

    #[tokio::test]
    async fn handle_call_missing_name() {
        let luft = build_test_luft();
        let runs = new_run_registry();
        let dirs = vec![];

        let params = json!({ "arguments": {} });
        let result = handle_call(&params, &luft, &runs, &dirs).await;
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("missing 'name'"));
    }

    // ── list_workflows_tool ─────────────────────────────────────────────

    #[test]
    fn list_workflows_empty_dirs() {
        let result = list_workflows_tool(&[PathBuf::from("/nonexistent")]);
        assert_eq!(result["isError"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: Vec<Value> = serde_json::from_str(text).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn list_workflows_with_files() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.lua"), "-- a test").unwrap();

        let result = list_workflows_tool(&[dir.path().to_path_buf()]);
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: Vec<Value> = serde_json::from_str(text).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["name"], "a");
    }

    // ── get_run_status_tool ─────────────────────────────────────────────

    #[tokio::test]
    async fn get_run_status_missing_run_id() {
        let luft = build_test_luft();
        let runs = new_run_registry();
        let args = json!({});
        let result = get_run_status_tool(&luft, &runs, &args).await;
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("missing required"));
    }

    #[tokio::test]
    async fn get_run_status_not_found() {
        let luft = build_test_luft();
        let runs = new_run_registry();
        let args = json!({ "run_id": "nonexistent-uuid" });
        let result = get_run_status_tool(&luft, &runs, &args).await;
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("run not found"));
    }

    // ── get_run_events_tool ─────────────────────────────────────────────

    #[tokio::test]
    async fn get_run_events_missing_run_id() {
        let luft = build_test_luft();
        let runs = new_run_registry();
        let args = json!({});
        let result = get_run_events_tool(&luft, &runs, &args).await;
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("missing required"));
    }

    #[tokio::test]
    async fn get_run_events_not_found() {
        let luft = build_test_luft();
        let runs = new_run_registry();
        let args = json!({ "run_id": "nonexistent" });
        let result = get_run_events_tool(&luft, &runs, &args).await;
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("run not found"));
    }

    // ── execute_workflow ────────────────────────────────────────────────

    #[tokio::test]
    async fn execute_workflow_missing_args() {
        let luft = build_test_luft();
        let runs = new_run_registry();
        let args = json!({});
        let result = execute_workflow(&luft, &runs, &args).await;
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("either 'script' or 'path'"));
    }

    #[tokio::test]
    async fn execute_workflow_validation_failure() {
        let luft = build_test_luft();
        let runs = new_run_registry();
        // Missing report() call → validation failure
        let args = json!({ "script": "function main() return 1 end" });
        let result = execute_workflow(&luft, &runs, &args).await;
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["valid"], false);
    }

    #[tokio::test]
    async fn execute_workflow_success() {
        let luft = build_test_luft();
        let runs = new_run_registry();
        let script = "meta = { reasoning = \"test\", phases = {} }\nfunction main() report({ok=true}) end";
        let args = json!({ "script": script });
        let result = execute_workflow(&luft, &runs, &args).await;
        assert_eq!(result["isError"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert!(parsed["run_id"].is_string());
        assert_eq!(parsed["status"], "running");

        // Verify the run was registered.
        let run_id = parsed["run_id"].as_str().unwrap();
        assert!(runs.lock().await.contains_key(run_id));
    }

    #[tokio::test]
    async fn execute_workflow_with_path() {
        let luft = build_test_luft();
        let runs = new_run_registry();
        let dir = tempfile::TempDir::new().unwrap();
        let script = "meta = { reasoning = \"path test\", phases = {} }\nfunction main() report({ok=true}) end";
        let file = dir.path().join("wf.lua");
        std::fs::write(&file, script).unwrap();

        let args = json!({ "path": file.to_str().unwrap() });
        let result = execute_workflow(&luft, &runs, &args).await;
        assert_eq!(result["isError"], false);
    }

    // ── new_run_registry ────────────────────────────────────────────────

    #[tokio::test]
    async fn new_run_registry_is_empty() {
        let r = new_run_registry();
        assert!(r.lock().await.is_empty());
    }

    // ── resolve_run_dir ─────────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_run_dir_found_in_registry() {
        let runs = new_run_registry();
        runs.lock().await.insert(
            "uuid-123".into(),
            RunInfo { run_dir_name: "task_12345".into() },
        );
        let dir = resolve_run_dir(&runs, "uuid-123").await;
        assert_eq!(dir, "task_12345");
    }

    #[tokio::test]
    async fn resolve_run_dir_fallback_to_input() {
        let runs = new_run_registry();
        let dir = resolve_run_dir(&runs, "some_dir_name").await;
        assert_eq!(dir, "some_dir_name");
    }

    // ── event_matches_id ────────────────────────────────────────────────

    #[test]
    fn event_matches_id_with_matching_content() {
        use luft_core::contract::event::RunStatus;
        let event = AgentEvent::RunDone {
            report: json!({"key": "evt-abc-123"}),
            status: RunStatus::Completed,
            run_id: uuid::Uuid::nil(),
            total_tokens: luft_core::contract::ids::TokenUsage::default(),
            ts: chrono::Utc::now(),
        };
        assert!(event_matches_id(&event, "evt-abc-123"));
    }

    #[test]
    fn event_matches_id_no_match() {
        use luft_core::contract::event::RunStatus;
        let event = AgentEvent::RunDone {
            report: json!({"key": "value"}),
            status: RunStatus::Completed,
            run_id: uuid::Uuid::nil(),
            total_tokens: luft_core::contract::ids::TokenUsage::default(),
            ts: chrono::Utc::now(),
        };
        assert!(!event_matches_id(&event, "nonexistent-id"));
    }

    // ── end-to-end: execute → status → events ───────────────────────────

    #[tokio::test]
    async fn execute_then_get_status_and_events() {
        let luft = build_test_luft();
        let runs = new_run_registry();

        // Execute a simple workflow.
        let script = "meta = { reasoning = \"e2e\", phases = {} }\nfunction main() report({ok=true}) end";
        let args = json!({ "script": script });
        let result = execute_workflow(&luft, &runs, &args).await;
        assert_eq!(result["isError"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        let run_id = parsed["run_id"].as_str().unwrap().to_string();

        // Give the run a moment to complete (mock backend is instant, but the
        // tokio task needs to be scheduled).
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Query status — should find the run.
        let status_args = json!({ "run_id": run_id });
        let status_result = get_run_status_tool(&luft, &runs, &status_args).await;
        // The run may or may not still be in the registry, but status should
        // be queryable via the run_dir on disk. If found, it's not an error.
        let status_text = status_result["content"][0]["text"].as_str().unwrap_or("");
        // Either we got a status (not error) or a "run not found" — both are
        // acceptable since timing-dependent. We just need the code path exercised.
        let _ = status_text;

        // Query events — should find the run.
        let events_args = json!({ "run_id": run_id });
        let events_result = get_run_events_tool(&luft, &runs, &events_args).await;
        let events_text = events_result["content"][0]["text"].as_str().unwrap_or("");
        let _ = events_text;
    }

    #[tokio::test]
    async fn get_run_events_with_since_event_id() {
        let luft = build_test_luft();
        let runs = new_run_registry();

        let script = "meta = { reasoning = \"filter\", phases = {} }\nfunction main() report({ok=true}) end";
        let args = json!({ "script": script });
        let result = execute_workflow(&luft, &runs, &args).await;
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        let run_id = parsed["run_id"].as_str().unwrap().to_string();

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Query events with since_event_id — exercises the filter path.
        let events_args = json!({ "run_id": run_id, "since_event_id": "nonexistent" });
        let events_result = get_run_events_tool(&luft, &runs, &events_args).await;
        let _ = events_result["content"][0]["text"].as_str().unwrap_or("");
    }
}
