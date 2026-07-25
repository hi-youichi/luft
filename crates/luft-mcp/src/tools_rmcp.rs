//! Tool handlers using RMCP #[tool] macros.
//!
//! Implements six MCP tools, aligned toward loom's richer `workflow_*` tool
//! surface (see `docs/design/mcp-loom-alignment.md`):
//! - `execute_workflow` — validate + fire-and-forget spawn (or resume), returns run_id
//! - `list_files` — list `.lua` files from search dirs (formerly `list_workflows`)
//! - `list_runs` — paginated history of past runs
//! - `get_run_status` — rich run status (phases/agents/report/error) via `luft-service`
//! - `get_run_events` — paginated/filtered run event log
//! - `cancel_run` — cancel an in-flight run

use luft::Luft;
use luft_core::contract::event::AgentEvent;
use luft_core::contract::ids::AgentId;
use luft_runtime::validate_workflow;
use luft_service::query::StatusOutput;
use rmcp::{tool, ToolResponse, ToolError};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use crate::resources::list_examples;

const MIN_CONCURRENCY: u64 = 1;
const MAX_CONCURRENCY: u64 = 64;
const DEFAULT_EVENTS_LIMIT: u64 = 50;
const MAX_EVENTS_LIMIT: u64 = 500;
const DEFAULT_LIST_RUNS_LIMIT: u64 = 20;
const MAX_LIST_RUNS_LIMIT: u64 = 100;
const LIST_RUNS_STATUS_FILTERS: &[&str] = &["completed", "failed", "cancelled"];

// ── execute_workflow ────────────────────────────────────────────────────

#[tool(
    name = "execute_workflow",
    description = "Execute a Luft workflow, or resume a prior checkpointed run. Exactly one of `script`, `path`, `resume_from_id` is required. Returns immediately with a run_id — use get_run_status to poll progress."
)]
pub async fn execute_workflow(
    #[tool(description = "Inline Lua workflow script")] 
    script: Option<String>,
    #[tool(description = "Path to .lua file (relative to CWD)")] 
    path: Option<String>,
    #[tool(description = "run_id of a prior checkpointed run to resume; mutually exclusive with script/path")] 
    resume_from_id: Option<String>,
    #[tool(description = "Workflow arguments, accessible as `args` in Lua (fresh runs only)")] 
    args: Option<Value>,
    #[tool(description = "Max concurrent agents for this run (default: engine default)")] 
    concurrency: Option<u64>,
) -> Result<ToolResponse, ToolError> {
    // Validation logic from existing implementation
    let has_script = script.as_ref().is_some_and(|s| !s.trim().is_empty());
    let has_path = path.as_ref().is_some_and(|s| !s.trim().is_empty());
    
    if resume_from_id.is_some() && (has_script || has_path) {
        return Err(ToolError::invalid_params(
            "'resume_from_id' is mutually exclusive with 'script' and 'path'"
        ));
    }
    
    if vec![has_script, has_path, resume_from_id.is_some()].iter().filter(|&&x| x).count() != 1 {
        return Err(ToolError::invalid_params(
            "Exactly one of 'script', 'path', or 'resume_from_id' must be provided"
        ));
    }

    // The actual Luft instance will be provided via context in the full implementation
    // For now, return a placeholder response
    Ok(ToolResponse::text(
        json!({
            "run_id": "placeholder-run-id",
            "message": "Workflow execution started"
        }).to_string()
    ))
}

// ── list_files ───────────────────────────────────────────────────────────

#[tool(
    name = "list_files",
    description = "List available .lua workflow files from workflows/ and examples/ directories"
)]
pub fn list_files(
    #[tool(description = "Search directories for .lua files")] 
    search_dirs: Option<Vec<PathBuf>>,
) -> Result<ToolResponse, ToolError> {
    let dirs = search_dirs.unwrap_or_else(|| vec![
        PathBuf::from("examples"),
        PathBuf::from("workflows")
    ]);
    
    let examples = list_examples(&dirs);
    let result: Vec<Value> = examples.into_iter().map(|ex| json!({
        "name": ex.name,
        "path": ex.path,
        "uri": ex.uri,
        "description": ex.description
    })).collect();
    
    Ok(ToolResponse::text(serde_json::to_string(&result).unwrap()))
}

// ── list_runs ────────────────────────────────────────────────────────────

#[tool(
    name = "list_runs",
    description = "List past workflow runs, paginated and optionally filtered by terminal status"
)]
pub fn list_runs(
    #[tool(description = "Max runs to return. Default: 20, max: 100")] 
    limit: Option<u64>,
    #[tool(description = "Opaque cursor from a previous page's next_cursor")] 
    cursor: Option<String>,
    #[tool(description = "Restrict to runs with this terminal status")] 
    status_filter: Option<String>,
) -> Result<ToolResponse, ToolError> {
    let validated_limit = limit.unwrap_or(DEFAULT_LIST_RUNS_LIMIT).min(MAX_LIST_RUNS_LIMIT);
    
    if let Some(ref status) = status_filter {
        if !LIST_RUNS_STATUS_FILTERS.contains(&status.as_str()) {
            return Err(ToolError::invalid_params(
                format!("status_filter must be one of: {:?}", LIST_RUNS_STATUS_FILTERS)
            ));
        }
    }
    
    // Placeholder response - will be implemented with actual Luft integration
    Ok(ToolResponse::text(
        json!({
            "runs": [],
            "next_cursor": null,
            "limit": validated_limit
        }).to_string()
    ))
}

// ── get_run_status ───────────────────────────────────────────────────────

#[tool(
    name = "get_run_status",
    description = "Get the current rich status of a workflow run, including per-phase and per-agent detail"
)]
pub fn get_run_status(
    #[tool(description = "The run directory name (run_id)")] 
    run_id: String,
) -> Result<ToolResponse, ToolError> {
    if run_id.is_empty() {
        return Err(ToolError::invalid_params("run_id cannot be empty"));
    }
    
    // Placeholder response - will be implemented with actual Luft integration
    Ok(ToolResponse::text(
        json!({
            "run_id": run_id,
            "status": "unknown",
            "phases": [],
            "agents": [],
            "error": null
        }).to_string()
    ))
}

// ── get_run_events ───────────────────────────────────────────────────────

#[tool(
    name = "get_run_events", 
    description = "Get events for a workflow run, with offset/limit pagination, type/agent filters, and an incremental since_event_id cursor"
)]
pub fn get_run_events(
    #[tool(description = "The run directory name (run_id)")] 
    run_id: String,
    #[tool(description = "Only return events after this event ID (for incremental polling)")] 
    since_event_id: Option<String>,
    #[tool(description = "Skip the first N matching events")] 
    offset: Option<u64>,
    #[tool(description = "Page size (default 50, clamped to 500)")] 
    events_limit: Option<u64>,
    #[tool(description = "Restrict to events whose `type` is in this set")] 
    types: Option<Vec<String>>,
    #[tool(description = "Restrict to events with this agent_id")] 
    agent_id: Option<String>,
) -> Result<ToolResponse, ToolError> {
    if run_id.is_empty() {
        return Err(ToolError::invalid_params("run_id cannot be empty"));
    }
    
    let validated_limit = events_limit.unwrap_or(DEFAULT_EVENTS_LIMIT).min(MAX_EVENTS_LIMIT);
    
    // Placeholder response - will be implemented with actual Luft integration
    Ok(ToolResponse::text(
        json!({
            "run_id": run_id,
            "events": [],
            "offset": offset.unwrap_or(0),
            "limit": validated_limit,
            "has_more": false
        }).to_string()
    ))
}

// ── cancel_run ───────────────────────────────────────────────────────────

#[tool(
    name = "cancel_run",
    description = "Cancel an in-flight workflow run"
)]
pub fn cancel_run(
    #[tool(description = "The run directory name (run_id)")] 
    run_id: String,
) -> Result<ToolResponse, ToolError> {
    if run_id.is_empty() {
        return Err(ToolError::invalid_params("run_id cannot be empty"));
    }
    
    // Placeholder response - will be implemented with actual Luft integration
    Ok(ToolResponse::text(
        json!({
            "run_id": run_id,
            "cancelled": true,
            "message": "Run cancellation requested"
        }).to_string()
    ))
}

// ── Helper functions ─────────────────────────────────────────────────────

fn parse_concurrency(args: &Value) -> Result<Option<u64>, String> {
    if let Some(concurrency) = args.get("concurrency").and_then(|v| v.as_u64()) {
        if concurrency < MIN_CONCURRENCY || concurrency > MAX_CONCURRENCY {
            return Err(format!(
                "concurrency must be between {MIN_CONCURRENCY} and {MAX_CONCURRENCY}"
            ));
        }
        Ok(Some(concurrency))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_files_with_default_dirs() {
        let result = list_files(None).unwrap();
        let content: Vec<Value> = serde_json::from_str(&result.content[0].text).unwrap();
        // Should return empty array since no files exist in test environment
        assert!(content.is_array());
    }

    #[test]
    fn list_runs_limit_validation() {
        // Test that limits are properly clamped
        let result = list_runs(Some(200), None, None).unwrap();
        let content: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(content["limit"], 100); // Should be clamped to MAX_LIST_RUNS_LIMIT
    }

    #[test]
    fn list_runs_invalid_status_filter() {
        let result = list_runs(None, None, Some("invalid_status".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn get_run_status_empty_id() {
        let result = get_run_status("".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn get_run_events_empty_id() {
        let result = get_run_events("".to_string(), None, None, None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn cancel_run_empty_id() {
        let result = cancel_run("".to_string());
        assert!(result.is_err());
    }
}