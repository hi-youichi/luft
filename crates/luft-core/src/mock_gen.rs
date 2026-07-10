//! Mock data generation for existing Lua workflow scripts.
//!
//! Reads a `.lua` script, extracts agent call info (name + schema), sends the
//! script to an LLM, and generates a `.mock.json` sidecar file.

use crate::contract::backend::{AgentBackend, AgentTask, BackendError, RunContext};
use crate::AgentStatus;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ── config ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MockGenConfig {
    pub model: Option<String>,
    pub max_retries: usize,
}

impl Default for MockGenConfig {
    fn default() -> Self {
        Self {
            model: None,
            max_retries: 3,
        }
    }
}

// ── agent call extraction ──────────────────────────────────────────

/// Information extracted from an `agent({...})` call in a Lua script.
#[derive(Debug, Clone)]
pub struct AgentCallInfo {
    pub name: Option<String>,
    pub schema_raw: Option<String>,
    pub line: usize,
}

/// Extract all `agent({...})` calls from a Lua script.
///
/// Performs brace-matching to find the full argument table for each call,
/// then looks for `name=` and `schema=` within.
pub fn extract_agent_calls(script: &str) -> Vec<AgentCallInfo> {
    let bytes = script.as_bytes();
    let mut calls = Vec::new();
    let mut pos = 0;

    while pos < script.len() {
        let rest = &script[pos..];
        let rel = match rest.find("agent") {
            Some(r) => r,
            None => break,
        };
        let agent_pos = pos + rel;

        let before_ok = agent_pos == 0 || !bytes[agent_pos - 1].is_ascii_alphabetic();
        if !before_ok {
            pos = agent_pos + 5;
            continue;
        }

        let after_agent = &script[agent_pos + 5..];
        let after_trimmed = after_agent.trim_start();
        if !after_trimmed.starts_with('(') {
            pos = agent_pos + 5;
            continue;
        }

        let paren_rel = after_agent.find('(').unwrap();
        let after_paren = &after_agent[paren_rel + 1..];
        let after_paren_trimmed = after_paren.trim_start();
        if !after_paren_trimmed.starts_with('{') {
            pos = agent_pos + 5;
            continue;
        }

        let brace_rel_in_paren = after_paren.find('{').unwrap();
        let brace_start = agent_pos + 5 + paren_rel + 1 + brace_rel_in_paren;

        let mut depth = 1i32;
        let mut i = brace_start + 1;
        while i < script.len() && depth > 0 {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            i += 1;
        }

        if depth != 0 {
            pos = agent_pos + 5;
            continue;
        }

        let arg_text = &script[brace_start + 1..i - 1];
        let line = script[..agent_pos].matches('\n').count() + 1;

        let name = extract_field_string(arg_text, "name");
        let schema_raw = extract_field_table(arg_text, "schema");

        calls.push(AgentCallInfo {
            name,
            schema_raw,
            line,
        });

        pos = i;
    }

    calls
}

/// Convenience: extract just the names from all agent calls.
pub fn extract_agent_names(script: &str) -> Vec<String> {
    extract_agent_calls(script)
        .into_iter()
        .filter_map(|c| c.name)
        .collect()
}

/// Find `<field> = "..."` or `<field> = '...'` in a text block.
fn extract_field_string(text: &str, field: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(rel) = text[from..].find(field) {
        let pos = from + rel;
        let before_ok = pos == 0
            || bytes[pos - 1].is_ascii_whitespace()
            || bytes[pos - 1] == b'{'
            || bytes[pos - 1] == b','
            || bytes[pos - 1] == b';';
        if !before_ok {
            from = pos + field.len();
            continue;
        }
        let after = &text[pos + field.len()..];
        let trimmed = after.trim_start();
        if !trimmed.starts_with('=') {
            from = pos + field.len();
            continue;
        }
        let after_eq = trimmed[1..].trim_start();
        let quote = match after_eq.chars().next() {
            Some('"') => '"',
            Some('\'') => '\'',
            _ => {
                from = pos + field.len();
                continue;
            }
        };
        let str_body = &after_eq[1..];
        if let Some(end) = str_body.find(quote) {
            return Some(str_body[..end].trim().to_string());
        }
        from = pos + field.len();
    }
    None
}

/// Find `<field> = { ... }` and return the raw table text (including braces).
fn extract_field_table(text: &str, field: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(rel) = text[from..].find(field) {
        let pos = from + rel;
        let before_ok = pos == 0
            || bytes[pos - 1].is_ascii_whitespace()
            || bytes[pos - 1] == b'{'
            || bytes[pos - 1] == b','
            || bytes[pos - 1] == b';';
        if !before_ok {
            from = pos + field.len();
            continue;
        }
        let after = &text[pos + field.len()..];
        let trimmed = after.trim_start();
        if !trimmed.starts_with('=') {
            from = pos + field.len();
            continue;
        }
        let after_eq = trimmed[1..].trim_start();
        if after_eq.starts_with('{') {
            let tb = after_eq.as_bytes();
            let mut depth = 1i32;
            let mut j = 1;
            while j < after_eq.len() && depth > 0 {
                match tb[j] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            if depth == 0 {
                return Some(after_eq[..j].to_string());
            }
        }
        from = pos + field.len();
    }
    None
}

// ── prompt construction ────────────────────────────────────────────

fn build_mock_prompt(script: &str, calls: &[AgentCallInfo], fix_error: Option<&str>) -> String {
    let names: Vec<&str> = calls.iter().filter_map(|c| c.name.as_deref()).collect();

    let mut p = String::with_capacity(script.len() + 2048);

    p.push_str(
        "You are given a Lua workflow script. Generate mock data for every named agent() call.\n\n",
    );
    p.push_str(&format!(
        "Agent names ({}): {}\n\n",
        names.len(),
        names.join(", ")
    ));

    let has_schemas = calls.iter().any(|c| c.schema_raw.is_some());
    if has_schemas {
        p.push_str("Schema hints:\n");
        for call in calls {
            if let (Some(name), Some(schema)) = (&call.name, &call.schema_raw) {
                p.push_str(&format!("  - \"{}\": {}\n", name, schema));
            }
        }
        p.push('\n');
    }

    p.push_str("--- Script ---\n```lua\n");
    p.push_str(script);
    p.push_str("\n```\n\n");

    p.push_str("Generate a ```json block with mock responses. Format:\n");
    p.push_str(
        r#"{
  "responses": {
    "<agent_name>": {
      "output": <representative JSON matching the agent's expected output>,
      "tokens": {"input": 100, "output": 50},
      "status": "ok"
    }
  },
  "default": {
    "output": {"text": "mock response"},
    "tokens": {"input": 0, "output": 0}
  }
}
"#,
    );

    p.push_str("\nRules:\n");
    p.push_str("1. Mock output MUST match the agent's schema if defined.\n");
    p.push_str("2. Keep output concise but structurally valid.\n");
    p.push_str("3. Every listed agent name MUST have a response.\n");
    p.push_str("4. For parallel() fan-out, all items share one name — one response.\n");
    p.push_str("5. Output ONLY the ```json block, no prose.\n");

    if let Some(err) = fix_error {
        p.push_str("\n# Previous attempt error\n\n");
        p.push_str(err);
        p.push_str("\n\nFix the JSON and try again.\n");
    }

    p
}

// ── LLM call + JSON extraction ─────────────────────────────────────

fn output_to_text(output: &serde_json::Value) -> Option<String> {
    match output {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => {
            for key in ["text", "message", "content", "output"] {
                if let Some(serde_json::Value::String(s)) = map.get(key) {
                    return Some(s.clone());
                }
            }
            Some(output.to_string())
        }
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn find_fenced_json(text: &str) -> Option<String> {
    let marker = "```json";
    let start = text.find(marker)? + marker.len();
    let body_start = text[start..].find('\n')? + start + 1;
    let rest = &text[body_start..];
    let end = rest.find("```")?;
    Some(rest[..end].trim_end().to_string())
}

// ── main entry point ───────────────────────────────────────────────

/// Generate mock data for an existing Lua script by calling an LLM.
///
/// Reads agent call info from the script, builds a prompt, calls the backend,
/// parses the JSON output, and validates coverage.
pub async fn generate_mock_for_script(
    script: &str,
    backend: Arc<dyn AgentBackend>,
    cfg: &MockGenConfig,
) -> Result<serde_json::Value> {
    let calls = extract_agent_calls(script);

    if calls.is_empty() {
        anyhow::bail!("no agent() calls found in script");
    }

    let unnamed: Vec<&AgentCallInfo> = calls.iter().filter(|c| c.name.is_none()).collect();
    if !unnamed.is_empty() {
        let lines: Vec<String> = unnamed
            .iter()
            .map(|c| format!("  line {}", c.line))
            .collect();
        anyhow::bail!(
            "found {} agent() calls without name=:\n{}\nEvery agent() call must include a unique name= field.",
            unnamed.len(),
            lines.join("\n")
        );
    }

    let names: Vec<String> = calls.iter().filter_map(|c| c.name.clone()).collect();

    let mut last_error = String::new();

    for attempt in 1..=cfg.max_retries {
        let prompt = build_mock_prompt(
            script,
            &calls,
            if attempt > 1 { Some(&last_error) } else { None },
        );

        let task = AgentTask {
            agent_id: Uuid::now_v7(),
            phase_id: 0,
            prompt,
            model: cfg.model.clone(),
            description: Some("Generate mock data for Lua workflow".into()),
            role: None,
            name: Some("mock_generator".into()),
            agent_seq: 0,
            allowlist: None,
            workdir: PathBuf::from("."),
            mcp_endpoint: None,
            timeout: None,
            output_schema: None,
        };

        let (events, _) = tokio::sync::broadcast::channel(16);
        let ctx = RunContext {
            run_id: Uuid::now_v7(),
            cancel: CancellationToken::new(),
            events,
        };

        let result = match backend.run(task, ctx).await {
            Ok(r) => r,
            Err(BackendError::Cancelled) => anyhow::bail!("cancelled"),
            Err(e) => {
                last_error = format!("backend error: {:?}", e);
                if attempt < cfg.max_retries {
                    tracing::warn!("attempt {}: {}", attempt, last_error);
                    continue;
                }
                anyhow::bail!(
                    "mock generation failed after {} attempts: {}",
                    cfg.max_retries,
                    last_error
                );
            }
        };

        if result.status != AgentStatus::Ok {
            last_error = format!("agent returned status {:?}", result.status);
            if attempt < cfg.max_retries {
                tracing::warn!("attempt {}: {}", attempt, last_error);
                continue;
            }
            anyhow::bail!("mock generation failed: {}", last_error);
        }

        let text = match output_to_text(&result.output) {
            Some(t) => t,
            None => {
                last_error = "agent returned empty output".into();
                continue;
            }
        };

        let json_str = match find_fenced_json(&text) {
            Some(s) => s,
            None => {
                let trimmed = text.trim();
                if trimmed.starts_with('{') {
                    trimmed.to_string()
                } else {
                    last_error = "no ```json block found in output".into();
                    continue;
                }
            }
        };

        let mock_data: serde_json::Value = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                last_error = format!("invalid JSON: {}", e);
                if attempt < cfg.max_retries {
                    tracing::warn!("attempt {}: {}", attempt, last_error);
                    continue;
                }
                anyhow::bail!(
                    "mock generation failed after {} attempts: {}",
                    cfg.max_retries,
                    last_error
                );
            }
        };

        let responses = mock_data.get("responses").and_then(|r| r.as_object());
        if let Some(responses) = responses {
            let has_default = mock_data.get("default").is_some();
            let missing: Vec<&str> = names
                .iter()
                .filter(|n| !responses.contains_key(n.as_str()))
                .map(|n| n.as_str())
                .collect();
            if !missing.is_empty() && !has_default {
                last_error = format!("missing mock responses for: {}", missing.join(", "));
                if attempt < cfg.max_retries {
                    tracing::warn!("attempt {}: {}", attempt, last_error);
                    continue;
                }
                anyhow::bail!("mock generation failed: {}", last_error);
            }
        }

        return Ok(mock_data);
    }

    anyhow::bail!(
        "mock generation exhausted {} attempts: {}",
        cfg.max_retries,
        last_error
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT_WITH_NAMES: &str = r#"
        meta = { reasoning = "test", phases = {} }
        function main()
            local r1 = agent({ name = "plan", prompt = "plan stuff" })
            local r2 = agent({ name = "analyze", prompt = "analyze", schema = { type = "object", properties = { text = { type = "string" } } } })
            report(r2)
        end
    "#;

    const SCRIPT_NO_NAMES: &str = r#"
        meta = { reasoning = "test", phases = {} }
        function main()
            local r = agent({ prompt = "do stuff" })
            report(r)
        end
    "#;

    #[test]
    fn extract_names_from_script() {
        let names = extract_agent_names(SCRIPT_WITH_NAMES);
        assert_eq!(names, vec!["plan", "analyze"]);
    }

    #[test]
    fn extract_names_empty_when_no_names() {
        let names = extract_agent_names(SCRIPT_NO_NAMES);
        assert!(names.is_empty());
    }

    #[test]
    fn extract_calls_finds_all() {
        let calls = extract_agent_calls(SCRIPT_WITH_NAMES);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name.as_deref(), Some("plan"));
        assert!(calls[0].schema_raw.is_none());
        assert_eq!(calls[1].name.as_deref(), Some("analyze"));
        assert!(calls[1].schema_raw.is_some());
        assert!(calls[1].schema_raw.as_ref().unwrap().contains("type"));
    }

    #[test]
    fn extract_calls_no_names() {
        let calls = extract_agent_calls(SCRIPT_NO_NAMES);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].name.is_none());
    }

    #[test]
    fn extract_calls_line_numbers() {
        let calls = extract_agent_calls(SCRIPT_WITH_NAMES);
        assert!(calls[0].line >= 3);
        assert!(calls[1].line >= 4);
    }

    #[test]
    fn extract_calls_with_nested_braces() {
        let script = r#"
            function main()
                agent({
                    name = "test",
                    schema = {
                        type = "object",
                        properties = {
                            items = {
                                type = "array",
                                items = { type = "string" }
                            }
                        }
                    }
                })
            end
        "#;
        let calls = extract_agent_calls(script);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name.as_deref(), Some("test"));
        let schema = calls[0].schema_raw.as_ref().unwrap();
        assert!(schema.contains("array"));
        assert!(schema.contains("items"));
        assert!(schema.ends_with('}'));
    }

    #[test]
    fn extract_calls_skips_non_agent_identifiers() {
        let script = "local x = my_agent_function({ name = \"x\" })";
        let calls = extract_agent_calls(script);
        assert!(calls.is_empty());
    }

    #[test]
    fn extract_field_string_handles_comma_prefix() {
        let text = "  , name = \"hello\"";
        assert_eq!(extract_field_string(text, "name"), Some("hello".into()));
    }

    #[test]
    fn extract_field_table_handles_boolean() {
        let text = "schema = true";
        assert_eq!(extract_field_table(text, "schema"), None);
    }

    #[test]
    fn build_prompt_contains_script_and_names() {
        let calls = extract_agent_calls(SCRIPT_WITH_NAMES);
        let p = build_mock_prompt(SCRIPT_WITH_NAMES, &calls, None);
        assert!(p.contains("plan"));
        assert!(p.contains("analyze"));
        assert!(p.contains("Agent names (2)"));
        assert!(p.contains("Schema hints"));
    }

    #[test]
    fn build_prompt_with_fix_error() {
        let calls = extract_agent_calls(SCRIPT_WITH_NAMES);
        let p = build_mock_prompt(SCRIPT_WITH_NAMES, &calls, Some("JSON was malformed"));
        assert!(p.contains("Previous attempt error"));
        assert!(p.contains("JSON was malformed"));
    }

    #[test]
    fn extract_field_string_handles_dynamic_concat() {
        let text = "name = \"analyze \" .. file.path";
        assert_eq!(extract_field_string(text, "name"), Some("analyze".into()));
    }

    #[test]
    fn extract_names_from_dynamic_concat() {
        let script = r#"
            function main()
                agent({ name = "analyze " .. file.path, prompt = "x" })
            end
        "#;
        let names = extract_agent_names(script);
        assert_eq!(names, vec!["analyze"]);
    }

    #[test]
    fn find_fenced_json_extracts_correctly() {
        let text = "Some prose\n```json\n{\"a\": 1}\n```\nMore prose";
        assert_eq!(find_fenced_json(text), Some("{\"a\": 1}".into()));
    }

    #[test]
    fn find_fenced_json_returns_none_when_absent() {
        assert!(find_fenced_json("no json here").is_none());
    }
}
