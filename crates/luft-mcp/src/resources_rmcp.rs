//! Resource handlers using RMCP resource traits.
//!
//! Provides three URI schemes:
//! - `workflow://schema` — embedded Lua DSL reference (static, compile-time)
//! - `workflow://examples` — dynamic list of example workflows (JSON)
//! - `workflow://example/{name}` — raw content of a single example `.lua` file

use anyhow::Result;
use rmcp::{resource, ResourceResponse, ResourceError};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::resources::list_examples;

// ── Static resource: schema ──────────────────────────────────────────────

#[resource(uri = "workflow://schema", name = "Workflow DSL Reference", description = "Complete Lua DSL syntax for writing Luft workflows")]
pub fn schema_resource() -> Result<ResourceResponse, ResourceError> {
    Ok(ResourceResponse::text(
        luft_planner::LUA_DSL_REFERENCE.to_string(),
        "text/markdown"
    ))
}

// ── Dynamic resource: examples list ────────────────────────────────────────

#[resource(uri = "workflow://examples", name = "Example Workflows", description = "List of available example workflows")]
pub fn examples_resource(
    #[resource(description = "Search directories for example files")] 
    search_dirs: Option<Vec<PathBuf>>,
) -> Result<ResourceResponse, ResourceError> {
    let dirs = search_dirs.unwrap_or_else(|| vec![
        PathBuf::from("examples"),
        PathBuf::from("workflows")
    ]);
    
    let examples = list_examples(&dirs);
    let json_content = serde_json::to_string_pretty(&examples)
        .map_err(|e| ResourceError::internal(format!("Failed to serialize examples: {}", e)))?;
    
    Ok(ResourceResponse::text(json_content, "application/json"))
}

// ── Dynamic resource: specific example ─────────────────────────────────────

#[resource(
    uri_template = "workflow://example/{name}",
    name = "Example Workflow",
    description = "Read a specific example workflow by name"
)]
pub fn example_resource(
    #[resource(description = "Name of the example workflow")] 
    name: String,
    #[resource(description = "Search directories for example files")] 
    search_dirs: Option<Vec<PathBuf>>,
) -> Result<ResourceResponse, ResourceError> {
    if name.is_empty() || name.contains('/') {
        return Err(ResourceError::invalid_params("Invalid example name"));
    }
    
    let dirs = search_dirs.unwrap_or_else(|| vec![
        PathBuf::from("examples"),
        PathBuf::from("workflows")
    ]);
    
    // Find the example file
    let path = find_example_file(&name, &dirs)
        .ok_or_else(|| ResourceError::not_found(format!("Example not found: {}", name)))?;
    
    let content = std::fs::read_to_string(&path)
        .map_err(|e| ResourceError::internal(format!("Failed to read file: {}", e)))?;
    
    Ok(ResourceResponse::text(content, "text/x-lua"))
}

// ── Helper functions ─────────────────────────────────────────────────────

/// Find a `.lua` file by stem name across `search_dirs`.
fn find_example_file(name: &str, search_dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in search_dirs {
        let candidate = dir.join(format!("{name}.lua"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn schema_resource_returns_markdown() {
        let result = schema_resource().unwrap();
        assert_eq!(result.mime_type, "text/markdown");
        assert!(!result.text.is_empty());
        assert!(result.text.contains("Workflow") || result.text.contains("workflow"));
    }

    #[test]
    fn examples_resource_empty_dirs() {
        let result = examples_resource(Some(vec![PathBuf::from("/nonexistent")])).unwrap();
        assert_eq!(result.mime_type, "application/json");
        let parsed: Vec<Value> = serde_json::from_str(&result.text).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn examples_resource_with_files() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("test.lua"),
            "meta = { reasoning = \"test\", phases = {} }\nfunction main() report('hi') end",
        ).unwrap();

        let result = examples_resource(Some(vec![dir.path().to_path_buf()])).unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result.text).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["name"], "test");
    }

    #[test]
    fn example_resource_found() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("demo.lua"), "return 42").unwrap();

        let result = example_resource("demo".to_string(), Some(vec![dir.path().to_path_buf()])).unwrap();
        assert_eq!(result.mime_type, "text/x-lua");
        assert_eq!(result.text, "return 42");
    }

    #[test]
    fn example_resource_not_found() {
        let result = example_resource("nonexistent".to_string(), Some(vec![PathBuf::from("/tmp")])));
        assert!(result.is_err());
    }

    #[test]
    fn example_resource_invalid_name() {
        let result = example_resource("".to_string(), None);
        assert!(result.is_err());
        
        let result = example_resource("path/with/slashes".to_string(), None);
        assert!(result.is_err());
    }
}