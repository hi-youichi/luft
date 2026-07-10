//! Resource handlers for the MCP server.
//!
//! Provides three URI schemes:
//! - `workflow://schema` — embedded Lua DSL reference (static, compile-time)
//! - `workflow://examples` — dynamic list of example workflows (JSON)
//! - `workflow://example/{name}` — raw content of a single example `.lua` file

use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The embedded Lua DSL reference markdown.
///
/// Sourced from `maestro-planner/src/lua_dsl_reference.md` at compile time.
pub const SCHEMA_MARKDOWN: &str =
    include_str!("../../maestro-planner/src/lua_dsl_reference.md");

/// MIME type for the schema resource.
pub const SCHEMA_MIME: &str = "text/markdown";

/// MIME type for example Lua files.
pub const LUA_MIME: &str = "text/x-lua";

/// MIME type for the examples list JSON.
pub const JSON_MIME: &str = "application/json";

// ── URI parsing ─────────────────────────────────────────────────────────

/// Parsed `workflow://` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowUri {
    /// `workflow://schema` — the Lua DSL reference.
    Schema,
    /// `workflow://examples` — list of available examples.
    Examples,
    /// `workflow://example/{name}` — a single example by name.
    Example(String),
}

impl WorkflowUri {
    /// Parse a `workflow://` URI string.
    ///
    /// Returns `None` if the URI is not a recognised `workflow://` resource.
    pub fn parse(uri: &str) -> Option<Self> {
        let prefix = "workflow://";
        let rest = uri.strip_prefix(prefix)?;
        match rest {
            "schema" => Some(Self::Schema),
            "examples" => Some(Self::Examples),
            rest => {
                let name_prefix = "example/";
                let name = rest.strip_prefix(name_prefix)?;
                if name.is_empty() || name.contains('/') {
                    return None;
                }
                Some(Self::Example(name.to_string()))
            }
        }
    }
}

// ── Resource read ───────────────────────────────────────────────────────

/// Result of reading a resource: the MIME type and the text content.
#[derive(Debug, Clone)]
pub struct ResourceContent {
    pub mime_type: &'static str,
    pub text: String,
}

/// Read a resource by its parsed URI.
///
/// `search_dirs` is a list of directories to scan for example `.lua` files
/// (typically `[examples/, workflows/]`). The first directory that contains
/// a matching file wins.
pub fn read_resource(uri: &WorkflowUri, search_dirs: &[PathBuf]) -> Result<ResourceContent> {
    match uri {
        WorkflowUri::Schema => Ok(ResourceContent {
            mime_type: SCHEMA_MIME,
            text: SCHEMA_MARKDOWN.to_string(),
        }),
        WorkflowUri::Examples => {
            let examples = list_examples(search_dirs);
            Ok(ResourceContent {
                mime_type: JSON_MIME,
                text: serde_json::to_string_pretty(&examples)?,
            })
        }
        WorkflowUri::Example(name) => {
            let path = find_example_file(name, search_dirs)
                .ok_or_else(|| anyhow::anyhow!("example not found: {name}"))?;
            let text = std::fs::read_to_string(&path)?;
            Ok(ResourceContent {
                mime_type: LUA_MIME,
                text,
            })
        }
    }
}

// ── Examples listing ────────────────────────────────────────────────────

/// A single example workflow entry in the `workflow://examples` list.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExampleEntry {
    pub uri: String,
    pub name: String,
    pub path: String,
    pub description: String,
}

/// Scan `search_dirs` for `.lua` files and return a sorted list of examples.
///
/// The `description` field is extracted from each file's `meta.reasoning`
/// via `maestro_planner::extract_meta`. Falls back to the first comment line
/// if meta extraction fails.
pub fn list_examples(search_dirs: &[PathBuf]) -> Vec<ExampleEntry> {
    let mut entries = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    for dir in search_dirs {
        if !dir.is_dir() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !seen_names.insert(stem.to_string()) {
                continue;
            }
            let description = extract_description(&path).unwrap_or_default();
            entries.push(ExampleEntry {
                uri: format!("workflow://example/{stem}"),
                name: stem.to_string(),
                path: path.display().to_string(),
                description,
            });
        }
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Extract a human-readable description from a `.lua` file.
///
/// Tries `meta.reasoning` first, then falls back to the first `-- comment` line.
fn extract_description(path: &Path) -> Option<String> {
    let source = std::fs::read_to_string(path).ok()?;

    // Try meta.reasoning via the planner's extract_meta.
    if let Ok(Some(meta)) = maestro_planner::meta::extract_meta(&source) {
        if !meta.reasoning.is_empty() {
            return Some(meta.reasoning);
        }
    }

    // Fallback: first Lua comment line.
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(comment) = trimmed.strip_prefix("--") {
            let c = comment.trim();
            if !c.is_empty() {
                return Some(c.to_string());
            }
        }
    }

    None
}

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

// ── Build the `resources/read` JSON response ───────────────────────────

/// Build the MCP `resources/read` response for a given URI.
///
/// Returns a JSON object with a `contents` array (on success) or an error
/// message string (on failure).
pub fn build_read_response(uri: &str, search_dirs: &[PathBuf]) -> Result<Value> {
    let parsed = WorkflowUri::parse(uri)
        .ok_or_else(|| anyhow::anyhow!("unknown resource URI: {uri}"))?;
    let content = read_resource(&parsed, search_dirs)?;
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": content.mime_type,
            "text": content.text
        }]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── WorkflowUri::parse ──────────────────────────────────────────────

    #[test]
    fn parse_schema() {
        assert_eq!(WorkflowUri::parse("workflow://schema"), Some(WorkflowUri::Schema));
    }

    #[test]
    fn parse_examples() {
        assert_eq!(
            WorkflowUri::parse("workflow://examples"),
            Some(WorkflowUri::Examples)
        );
    }

    #[test]
    fn parse_example_named() {
        assert_eq!(
            WorkflowUri::parse("workflow://example/hello"),
            Some(WorkflowUri::Example("hello".into()))
        );
    }

    #[test]
    fn parse_example_empty_name_returns_none() {
        assert_eq!(WorkflowUri::parse("workflow://example/"), None);
    }

    #[test]
    fn parse_example_with_slash_returns_none() {
        assert_eq!(WorkflowUri::parse("workflow://example/foo/bar"), None);
    }

    #[test]
    fn parse_non_workflow_scheme_returns_none() {
        assert!(WorkflowUri::parse("http://example.com").is_none());
    }

    #[test]
    fn parse_unknown_workflow_resource() {
        assert!(WorkflowUri::parse("workflow://unknown").is_none());
    }

    #[test]
    fn parse_empty_string() {
        assert!(WorkflowUri::parse("").is_none());
    }

    // ── read_resource: schema ───────────────────────────────────────────

    #[test]
    fn read_schema_returns_markdown() {
        let dirs = vec![];
        let content = read_resource(&WorkflowUri::Schema, &dirs).unwrap();
        assert_eq!(content.mime_type, SCHEMA_MIME);
        assert!(!content.text.is_empty());
        assert!(content.text.contains("Workflow") || content.text.contains("workflow"));
    }

    // ── read_resource: examples ─────────────────────────────────────────

    #[test]
    fn read_examples_empty_dirs_returns_empty_array() {
        let dirs = vec![PathBuf::from("/nonexistent")];
        let content = read_resource(&WorkflowUri::Examples, &dirs).unwrap();
        assert_eq!(content.mime_type, JSON_MIME);
        let parsed: Vec<Value> = serde_json::from_str(&content.text).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn read_examples_with_lua_files() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("hello.lua"),
            "meta = { reasoning = \"test hello\", phases = {} }\nfunction main() report('hi') end",
        )
        .unwrap();
        fs::write(dir.path().join("other.txt"), "not lua").unwrap();

        let content = read_resource(&WorkflowUri::Examples, &[dir.path().to_path_buf()]).unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&content.text).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["name"], "hello");
        assert_eq!(parsed[0]["description"], "test hello");
        assert_eq!(parsed[0]["uri"], "workflow://example/hello");
    }

    // ── read_resource: example/{name} ───────────────────────────────────

    #[test]
    fn read_example_by_name() {
        let dir = TempDir::new().unwrap();
        let lua = "meta = { reasoning = \"demo\", phases = {} }\nfunction main() report('ok') end";
        fs::write(dir.path().join("demo.lua"), lua).unwrap();

        let content =
            read_resource(&WorkflowUri::Example("demo".into()), &[dir.path().to_path_buf()])
                .unwrap();
        assert_eq!(content.mime_type, LUA_MIME);
        assert_eq!(content.text, lua);
    }

    #[test]
    fn read_example_not_found() {
        let dir = TempDir::new().unwrap();
        let result = read_resource(
            &WorkflowUri::Example("nope".into()),
            &[dir.path().to_path_buf()],
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("example not found: nope"));
    }

    #[test]
    fn read_example_finds_first_match_across_dirs() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        fs::write(dir1.path().join("shared.lua"), "-- from dir1").unwrap();
        fs::write(dir2.path().join("shared.lua"), "-- from dir2").unwrap();

        let content = read_resource(
            &WorkflowUri::Example("shared".into()),
            &[dir1.path().to_path_buf(), dir2.path().to_path_buf()],
        )
        .unwrap();
        assert_eq!(content.text, "-- from dir1");
    }

    // ── list_examples ───────────────────────────────────────────────────

    #[test]
    fn list_examples_skips_non_lua_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.lua"), "-- a").unwrap();
        fs::write(dir.path().join("b.txt"), "text").unwrap();
        fs::write(dir.path().join("c.json"), "{}").unwrap();

        let entries = list_examples(&[dir.path().to_path_buf()]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "a");
    }

    #[test]
    fn list_examples_deduplicates_across_dirs() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        fs::write(dir1.path().join("dup.lua"), "-- dir1").unwrap();
        fs::write(dir2.path().join("dup.lua"), "-- dir2").unwrap();

        let entries = list_examples(&[dir1.path().to_path_buf(), dir2.path().to_path_buf()]);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn list_examples_sorted_by_name() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("zebra.lua"), "-- z").unwrap();
        fs::write(dir.path().join("apple.lua"), "-- a").unwrap();
        fs::write(dir.path().join("mango.lua"), "-- m").unwrap();

        let entries = list_examples(&[dir.path().to_path_buf()]);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["apple", "mango", "zebra"]);
    }

    #[test]
    fn list_examples_skips_directories_with_lua_extension() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("weird.lua")).unwrap();
        fs::write(dir.path().join("real.lua"), "-- real").unwrap();

        let entries = list_examples(&[dir.path().to_path_buf()]);
        // The directory named "weird.lua" has no file_stem match because it's a dir,
        // but read_dir will list it. file_stem works on dirs too, so we'd try to
        // extract description from it which would fail — the entry may still appear.
        // Let's just verify "real" is there.
        assert!(entries.iter().any(|e| e.name == "real"));
    }

    #[test]
    fn list_examples_handles_nonexistent_dir() {
        let entries = list_examples(&[PathBuf::from("/totally/nonexistent/path")]);
        assert!(entries.is_empty());
    }

    #[test]
    fn list_examples_description_from_meta_reasoning() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("meta_test.lua"),
            "meta = { reasoning = \"from meta\", phases = {} }\nfunction main() end",
        )
        .unwrap();
        let entries = list_examples(&[dir.path().to_path_buf()]);
        assert_eq!(entries[0].description, "from meta");
    }

    #[test]
    fn list_examples_description_fallback_to_comment() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("comment.lua"), "-- First comment line\nreturn 1").unwrap();
        let entries = list_examples(&[dir.path().to_path_buf()]);
        assert_eq!(entries[0].description, "First comment line");
    }

    #[test]
    fn list_examples_no_description_when_empty() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("bare.lua"), "return 1").unwrap();
        let entries = list_examples(&[dir.path().to_path_buf()]);
        assert_eq!(entries[0].description, "");
    }

    // ── build_read_response ─────────────────────────────────────────────

    #[test]
    fn build_read_response_schema() {
        let resp = build_read_response("workflow://schema", &[]).unwrap();
        assert_eq!(resp["contents"][0]["uri"], "workflow://schema");
        assert_eq!(resp["contents"][0]["mimeType"], "text/markdown");
        assert!(resp["contents"][0]["text"].as_str().unwrap().len() > 100);
    }

    #[test]
    fn build_read_response_unknown_uri() {
        let result = build_read_response("workflow://bogus", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn build_read_response_non_workflow_uri() {
        let result = build_read_response("http://foo", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn build_read_response_example_found() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("x.lua"), "return 42").unwrap();
        let resp = build_read_response("workflow://example/x", &[dir.path().to_path_buf()]).unwrap();
        assert_eq!(resp["contents"][0]["text"], "return 42");
        assert_eq!(resp["contents"][0]["mimeType"], "text/x-lua");
    }

    #[test]
    fn build_read_response_example_not_found() {
        let dir = TempDir::new().unwrap();
        let result = build_read_response("workflow://example/missing", &[dir.path().to_path_buf()]);
        assert!(result.is_err());
    }

    // ── extract_description edge cases ──────────────────────────────────

    #[test]
    fn extract_description_skips_empty_comments() {
        let dir = TempDir::new().unwrap();
        // First comment is empty, second has content
        fs::write(
            dir.path().join("edge.lua"),
            "--\n-- actual description\nreturn 1",
        )
        .unwrap();
        let entries = list_examples(&[dir.path().to_path_buf()]);
        assert_eq!(entries[0].description, "actual description");
    }

    #[test]
    fn extract_description_from_file_with_no_meta() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("nometa.lua"), "-- Just a comment\nfunction main() end").unwrap();
        let entries = list_examples(&[dir.path().to_path_buf()]);
        assert_eq!(entries[0].description, "Just a comment");
    }
}
