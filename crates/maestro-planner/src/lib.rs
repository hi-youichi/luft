//! `planner` — NL → Lua planner.
//!
//! Aligns with Claude Code Dynamic Workflows: instead of classifying the task
//! with keywords and filling fixed templates, the planner asks an LLM *agent*
//! to generate a Lua orchestration script for the task. The model acts as a
//! compiler (NL → DSL); the runtime then executes the script deterministically.
//!
//! The generated script orchestrates only — it never touches the filesystem or
//! shell (the Lua sandbox forbids `io`/`os`, see [`crate::runtime`]). All real
//! work — reading files, grepping, editing, web search — happens inside the
//! `agent()` prompts the script spawns at runtime.

use maestro_core::contract::backend::{AgentBackend, AgentTask, RunContext};
use maestro_core::contract::event::AgentEvent;
use maestro_runtime::{validate_script, validate_workflow};
use std::path::PathBuf;
use std::sync::Arc;

/// Planning configuration.
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    /// Model used by the planner agent (`None` = backend default).
    pub planner_model: Option<String>,
    /// Max attempts to (re)generate a valid script before giving up.
    pub max_retries: usize,
    /// When true, the planner also generates a `.mock.json` companion block.
    pub generate_mock: bool,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            planner_model: None,
            max_retries: 3,
            generate_mock: false,
        }
    }
}

/// A planned workflow: the generated Lua orchestration script.
#[derive(Debug, Clone)]
pub struct PlannedWorkflow {
    /// The generated Lua script (validated, fence-stripped).
    pub script: String,
    /// Mock data for `--with-mock` runs (`None` when mock generation is off).
    pub mock_data: Option<serde_json::Value>,
}

/// Planner errors.
#[derive(thiserror::Error, Debug)]
pub enum PlannerError {
    /// The backend agent call itself failed.
    #[error("planner backend error: {0}")]
    Backend(String),
    /// Ran out of attempts without producing a valid script.
    #[error("planner exhausted {attempts} attempt(s); last error: {last_error}")]
    ExhaustedRetries { attempts: usize, last_error: String },
}

/// Generate a Lua orchestration script for `task` by asking the backend agent.
///
/// Retries up to `cfg.max_retries` times, feeding the validation error back to
/// the agent so it can self-correct.
pub async fn plan_workflow(
    task: &str,
    backend: Arc<dyn AgentBackend>,
    cfg: &PlannerConfig,
) -> Result<PlannedWorkflow, PlannerError> {
    let attempts = cfg.max_retries.max(1);
    let mut last_error = String::new();

    for attempt in 0..attempts {
        if attempt > 0 {
            tracing::warn!(
                attempt,
                total = attempts,
                "retrying script generation after validation failure"
            );
        }

        let prompt = build_prompt(
            task,
            (attempt > 0).then_some(last_error.as_str()),
            cfg.generate_mock,
        );

        let output = run_planner_agent(&*backend, &prompt, cfg.planner_model.clone())
            .await
            .map_err(PlannerError::Backend)?;

        let script = match extract_script(&output) {
            Some(s) => s,
            None => {
                tracing::warn!(attempt, "agent output contained no lua code block");
                last_error = "no ```lua code block (or text) found in agent output".to_string();
                continue;
            }
        };

        match validate_generated(&script) {
            Ok(()) => {
                let mock_data = if cfg.generate_mock {
                    let mock = extract_mock_block(&output);
                    if mock.is_none() {
                        tracing::warn!(
                            "generate_mock requested but no ```json block found in agent output"
                        );
                    }
                    mock
                } else {
                    None
                };
                return Ok(PlannedWorkflow { script, mock_data });
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "generated script failed validation");
                last_error = e;
            }
        }
    }

    Err(PlannerError::ExhaustedRetries {
        attempts,
        last_error,
    })
}

/// Run a single planning agent through the backend, returning its raw output.
async fn run_planner_agent(
    backend: &dyn AgentBackend,
    prompt: &str,
    model: Option<String>,
) -> Result<serde_json::Value, String> {
    // The planner needs a one-shot RunContext; events are discarded.
    let (events, _rx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let ctx = RunContext {
        run_id: uuid::Uuid::now_v7(),
        cancel: tokio_util::sync::CancellationToken::new(),
        events,
    };
    let task = AgentTask {
        agent_id: uuid::Uuid::now_v7(),
        phase_id: 0,
        prompt: prompt.to_string(),
        model,
        description: None,
        role: None,
        name: None,
        agent_seq: 0,
        allowlist: None,
        workdir: PathBuf::from("."),
        mcp_endpoint: None,
        timeout: None,
        output_schema: None,
    };
    backend
        .run(task, ctx)
        .await
        .map(|r| r.output)
        .map_err(|e| e.to_string())
}

/// Validate a generated script: syntax + structure + heuristic checks.
fn validate_generated(script: &str) -> Result<(), String> {
    let result = validate_workflow(script).map_err(|e| format!("lua validation error: {}", e))?;
    if result.errors.is_empty() {
        Ok(())
    } else {
        Err(result.errors.join("; "))
    }
}

/// Extract the ```json block from planner output (for mock data).
fn extract_mock_block(output: &serde_json::Value) -> Option<serde_json::Value> {
    let text = output_to_text(output)?;
    let json_str = find_fenced_block_by_lang(&text, "json")?;
    let trimmed = json_str.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

/// Find a fenced code block by exact language tag.
fn find_fenced_block_by_lang(text: &str, lang: &str) -> Option<String> {
    let marker = format!("```{}", lang);
    let mut from = 0;
    while let Some(rel) = text[from..].find(&marker) {
        let fence = from + rel;
        let after = &text[fence + marker.len()..];
        let line_end = match after.find('\n') {
            Some(n) => n,
            None => {
                from = fence + marker.len();
                continue;
            }
        };
        let body_start = fence + marker.len() + line_end + 1;
        let rest = &text[body_start..];
        let close = match rest.find("```") {
            Some(c) => c,
            None => {
                from = body_start;
                continue;
            }
        };
        return Some(rest[..close].trim_end().to_string());
    }
    None
}

/// Coerce the agent output into text and pull out the Lua script.
fn extract_script(output: &serde_json::Value) -> Option<String> {
    let text = output_to_text(output)?;
    let block = extract_lua_block(&text);
    if block.trim().is_empty() {
        None
    } else {
        Some(block)
    }
}

/// Best-effort extraction of assistant text from a backend's structured output.
///
/// The exact shape depends on the backend (real ACP backends land in P0-A); this
/// handles the common cases and falls back to a JSON stringification.
fn output_to_text(output: &serde_json::Value) -> Option<String> {
    match output {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => {
            for key in ["script", "message", "text", "content", "output"] {
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

/// Return the first ```lua (or unlabelled) fenced block; otherwise try to
/// strip surrounding prose from the trimmed input.
fn extract_lua_block(text: &str) -> String {
    if let Some(block) = find_fenced_block(text) {
        return block;
    }
    let trimmed = text.trim();
    if validate_script(trimmed).is_ok() {
        return trimmed.to_string();
    }
    strip_prose(trimmed)
}

/// Heuristically strip leading/trailing prose lines that aren't valid Lua.
/// Keeps lines from the first Lua-looking line to the last.
fn strip_prose(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let lua_start = lines.iter().position(|l| looks_like_lua(l)).unwrap_or(0);
    let lua_end = lines
        .iter()
        .rposition(|l| looks_like_lua(l))
        .map(|i| i + 1)
        .unwrap_or(lines.len());
    lines[lua_start.min(lua_end.max(1))..lua_end.max(lines.len().min(lua_start + 1))]
        .join("\n")
        .trim()
        .to_string()
}

/// Check if a line looks like it could be Lua code (not prose).
fn looks_like_lua(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    if t.starts_with("--") {
        return true;
    }
    const KEYWORDS: &[&str] = &[
        "local", "phase", "report", "agent", "parallel", "pipeline", "for", "if", "while",
        "function", "return", "log", "budget", "workflow", "json", "do", "end", "else", "elseif",
        "then", "and", "or", "not", "true", "false", "nil", "args", "ctx",
    ];
    let first_word = t
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("");
    KEYWORDS.contains(&first_word)
        || t.starts_with('{')
        || t.starts_with('}')
        || t.starts_with('(')
        || t.starts_with(')')
        || t.starts_with('"')
        || t.starts_with('\'')
        || t.starts_with('[')
        || t.starts_with(']')
        || t.starts_with('=')
        || t.starts_with('.')
        || t.starts_with(':')
        || t.starts_with('#')
}

fn find_fenced_block(text: &str) -> Option<String> {
    let mut from = 0;
    while let Some(rel) = text[from..].find("```") {
        let fence = from + rel;
        let after = &text[fence + 3..];
        let line_end = match after.find('\n') {
            Some(n) => n,
            None => {
                from = fence + 3;
                continue;
            }
        };
        let lang = after[..line_end].trim();
        let body_start = fence + 3 + line_end + 1;
        let rest = &text[body_start..];
        let close = match rest.find("```") {
            Some(c) => c,
            None => {
                from = body_start;
                continue;
            }
        };
        if lang.eq_ignore_ascii_case("lua") || lang.is_empty() {
            return Some(rest[..close].trim_end().to_string());
        }
        from = body_start + close + 3;
    }
    None
}

/// Build the planner prompt: DSL reference + task (+ optional fix-up error).
fn build_prompt(task: &str, fix_error: Option<&str>, generate_mock: bool) -> String {
    let mut p = String::with_capacity(LUA_DSL_REFERENCE.len() + task.len() + 1024);
    p.push_str(LUA_DSL_REFERENCE);
    p.push_str("\n\n# Task\n\n");
    p.push_str(task);
    p.push('\n');
    if let Some(err) = fix_error {
        p.push_str("\n# Your previous attempt was rejected\n\n");
        p.push_str(err);
        p.push_str("\n\nFix the script and output a corrected version.\n");
    }
    if generate_mock {
        p.push_str(MOCK_GENERATION_INSTRUCTIONS);
    }
    p.push_str("\nOutput ONLY one ```lua code block");
    if generate_mock {
        p.push_str(" followed by one ```json code block");
    }
    p.push_str(" — no prose before or after.\n");
    p
}

const MOCK_GENERATION_INSTRUCTIONS: &str = r#"

# Mock Data Generation

In ADDITION to the ```lua block, output a second ```json block with mock
responses for EVERY named agent call. Format:

```json
{
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
```

Rules:
1. EVERY agent() call in the Lua script MUST include a unique `name=` field.
2. For parallel() fan-out, all items share ONE name — the mock response is reused.
3. Mock output MUST match the agent's schema structure if a schema is defined.
4. Keep mock output concise but structurally valid.
5. Output BOTH blocks: ```lua first, then ```json.
"#;

/// The orchestration DSL spec handed to the planner agent.
///
/// ## Role
///
/// This constant is the **system prompt** for the planner LLM. When a user runs
/// `maestro run --nl "<task>"`, the planner sends this text + the user's task
/// description to the LLM, and expects a single ```lua code block back. The
/// returned script is then validated (syntax + `report()` presence + span pairing)
/// and executed by the sandboxed [`crate::runtime`].
///
/// ## Architecture: NL → Lua compilation
///
/// ```text
///   User NL task ──► planner LLM ──► Lua script ──► sandbox execute
///        │              ▲                              │
///        │              │                              ▼
///        └──── LUA_DSL_REFERENCE (this const)     agent() calls ──► scheduler
///                                                     │
///                                                     ▼
///                                                   report()
/// ```
///
/// The LLM acts as a "compiler" from natural language to the Maestro Lua DSL.
/// The script is pure orchestration — no I/O, no filesystem, no shell. All real
/// work (file reads, grep, edit, web search) happens inside the `agent()` prompts.
///
/// ## Keeping in sync
///
/// Every primitive documented here MUST be registered in [`maestro_runtime::sandbox`]
/// via `register_sdk()`. If you add a new Lua global, document it here and register
/// it there. If you remove one, remove it from both places.
///
/// ## Token cost
///
/// This reference is sent on every planner call (~4K tokens). Keep examples
/// concise but illustrative. Rules are authoritative — primitives are reference.
const LUA_DSL_REFERENCE: &str = include_str!("lua_dsl_reference.md");

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_core::{FailKind, MockBackend, MockBehavior, TokenUsage};
    use std::time::Duration;

    fn mock_returning(output: serde_json::Value) -> Arc<dyn AgentBackend> {
        Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output,
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        ))
    }

    #[tokio::test]
    async fn test_plan_extracts_and_validates_script() {
        let script = "```lua\nmeta = { reasoning = \"test\", phases = {{ label = \"work\" }} }\nfunction main()\n  local r = agent({prompt='hi'})\n  report({ok=true})\nend\n```";
        let backend = mock_returning(serde_json::json!(script));
        let planned = plan_workflow("do something", backend, &PlannerConfig::default())
            .await
            .unwrap();
        assert!(planned.script.contains("report("));
        assert!(!planned.script.contains("```"));
        assert!(validate_script(&planned.script).is_ok());
    }

    #[tokio::test]
    async fn test_plan_retries_on_invalid_then_fails() {
        // Unbalanced parenthesis → syntax error, repeated for every retry.
        let bad = "```lua\nlocal x = (\nreport({})\n```";
        let backend = mock_returning(serde_json::json!(bad));
        let cfg = PlannerConfig {
            planner_model: None,
            max_retries: 2,
            ..Default::default()
        };
        match plan_workflow("x", backend, &cfg).await.unwrap_err() {
            PlannerError::ExhaustedRetries { attempts, .. } => assert_eq!(attempts, 2),
            other => panic!("expected ExhaustedRetries, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_plan_rejects_missing_report() {
        // Valid Lua but no report() call → rejected, retries exhausted.
        let no_report = "```lua\nlocal r = agent({prompt='hi'})\n```";
        let backend = mock_returning(serde_json::json!(no_report));
        let cfg = PlannerConfig {
            planner_model: None,
            max_retries: 1,
            ..Default::default()
        };
        assert!(matches!(
            plan_workflow("x", backend, &cfg).await,
            Err(PlannerError::ExhaustedRetries { .. })
        ));
    }

    #[test]
    fn test_extract_script_fenced_bare_and_object() {
        // Fenced lua block with surrounding prose.
        let v = serde_json::json!("prefix\n```lua\nreport({})\n```\nsuffix");
        assert_eq!(extract_script(&v).unwrap().trim(), "report({})");

        // Bare text (no fence) → trimmed.
        let v = serde_json::json!("  report({})  ");
        assert_eq!(extract_script(&v).unwrap(), "report({})");

        // Object with a `message` field.
        let v = serde_json::json!({ "message": "```lua\nreport({})\n```" });
        assert_eq!(extract_script(&v).unwrap().trim(), "report({})");

        // Null → None.
        assert!(extract_script(&serde_json::Value::Null).is_none());
    }

    #[test]
    fn test_extract_skips_non_lua_block() {
        let v = serde_json::json!("```text\nnot code\n```\n```lua\nreport({})\n```");
        assert_eq!(extract_script(&v).unwrap().trim(), "report({})");
    }

    // ── Additional coverage: backend error path ──────────────────────────

    #[tokio::test]
    async fn test_plan_backend_error() {
        let backend: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::fail(FailKind::Protocol)],
        ));
        let err = plan_workflow("x", backend, &PlannerConfig::default())
            .await
            .unwrap_err();
        assert!(matches!(err, PlannerError::Backend(_)));
    }

    // ── Coverage: agent output with no lua block (extract_script → None) ─

    #[tokio::test]
    async fn test_plan_no_lua_block_retries() {
        let backend = mock_returning(serde_json::Value::Null);
        let cfg = PlannerConfig {
            max_retries: 2,
            ..Default::default()
        };
        let err = plan_workflow("x", backend, &cfg).await.unwrap_err();
        assert!(matches!(err, PlannerError::ExhaustedRetries { .. }));
    }

    // ── Coverage: validation failure followed by successful retry ────────

    #[tokio::test]
    async fn test_plan_retry_then_succeeds() {
        let valid = "```lua\nmeta = { reasoning = \"test\", phases = {{ label = \"work\" }} }\nfunction main()\n  local r = agent({prompt='hi'})\n  report({ok=true})\nend\n```";
        let backend: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(
            "mock",
            vec![
                MockBehavior::Success {
                    output: serde_json::json!("this is pure garbage"),
                    tokens: TokenUsage::default(),
                    delay: Duration::ZERO,
                },
                MockBehavior::Success {
                    output: serde_json::json!(valid),
                    tokens: TokenUsage::default(),
                    delay: Duration::ZERO,
                },
            ],
        ));
        let planned = plan_workflow("x", backend, &PlannerConfig::default())
            .await
            .unwrap();
        assert!(planned.script.contains("report({ok=true})"));
    }

    // ── Coverage: strip_prose edge cases (lines 191, 195, 201) ──────────

    #[test]
    fn test_strip_prose_edge_cases() {
        // No lines look like Lua → lua_start = 0, lua_end = lines.len()
        let s = strip_prose("hello world\nsome prose");
        assert_eq!(s, "hello world\nsome prose");

        // Lua in middle, prose before/after → slices correctly
        let s = strip_prose("intro\nlocal x = 1\noutro");
        assert_eq!(s, "local x = 1");

        // Only Lua lines
        let s = strip_prose("local x = 1\nreport({})");
        assert_eq!(s, "local x = 1\nreport({})");

        // Lines with leading/trailing whitespace → trim exercised
        let s = strip_prose("  local x = 1  ");
        assert_eq!(s, "local x = 1");
    }

    // ── Coverage: looks_like_lua branches ────────────────────────────────

    #[test]
    fn test_looks_like_lua_variants() {
        assert!(!looks_like_lua(""));
        assert!(!looks_like_lua("   "));
        assert!(looks_like_lua("-- comment"));
        assert!(looks_like_lua("local x = 1"));
        assert!(looks_like_lua("{hello}"));
        assert!(looks_like_lua("}"));
        assert!(looks_like_lua("(arg)"));
        assert!(looks_like_lua(")"));
        assert!(looks_like_lua("\"string\""));
        assert!(looks_like_lua("'string'"));
        assert!(looks_like_lua("[1, 2]"));
        assert!(looks_like_lua("]"));
        assert!(looks_like_lua("= value"));
        assert!(looks_like_lua(".method"));
        assert!(looks_like_lua(":method"));
        assert!(looks_like_lua("#list"));
        assert!(!looks_like_lua("plain prose text"));
        assert!(!looks_like_lua("1234 number line"));
    }

    // ── Coverage: find_fenced_block edge cases ──────────────────────────

    #[test]
    fn test_find_fenced_block_edge_cases() {
        // Non-lua fence skipped, lua fence found
        let s = find_fenced_block("```text\nstuff\n```\n```lua\nreport({})\n```");
        assert_eq!(s.unwrap().trim(), "report({})");

        // Missing closing fence → None
        assert!(find_fenced_block("```lua\nreport({})").is_none());

        // No fence at all
        assert!(find_fenced_block("just text").is_none());
    }

    // ── Coverage: output_to_text remaining match arms ───────────────────

    #[test]
    fn test_output_to_text_variants() {
        // Object with no recognized key → fallback to_string
        let obj = serde_json::json!({"unknown": "value"});
        assert!(output_to_text(&obj).is_some());

        // Array → `other` arm
        assert_eq!(
            output_to_text(&serde_json::json!([1, 2, 3])).unwrap(),
            "[1,2,3]"
        );

        // Number → `other` arm
        assert_eq!(output_to_text(&serde_json::json!(42)).unwrap(), "42");
    }

    // ── Coverage: build_prompt fix_error branch ─────────────────────────

    #[test]
    fn test_build_prompt_with_fix_error() {
        let p = build_prompt("do task", Some("script had syntax error"), false);
        assert!(p.contains("do task"));
        assert!(p.contains("previous attempt was rejected"));
        assert!(p.contains("script had syntax error"));
        assert!(p.contains("Output ONLY"));
    }

    #[test]
    fn test_build_prompt_without_fix_error() {
        let p = build_prompt("do task", None, false);
        assert!(p.contains("do task"));
        assert!(!p.contains("previous attempt was rejected"));
        assert!(p.contains("Output ONLY"));
    }

    // ── Span pairing validation ─────────────────────────────────────────

    #[test]
    fn test_validate_span_unpaired() {
        let script = "meta = { reasoning = \"test\", phases = {} }\nfunction main()\nlocal m = phase_begin(\"x\")\nreport({})\nend";
        assert!(validate_generated(script).is_err());
    }

    #[test]
    fn test_validate_span_paired() {
        let script = "meta = { reasoning = \"test\", phases = {} }\nfunction main()\nlocal m = phase_begin(\"x\")\nphase_end(m)\nreport({})\nend";
        assert!(validate_generated(script).is_ok());
    }

    #[test]
    fn test_validate_no_span_ok() {
        let script =
            "meta = { reasoning = \"test\", phases = {} }\nfunction main() report({ok=true}) end";
        assert!(validate_generated(script).is_ok());
    }

    #[test]
    fn test_build_prompt_contains_decomposition_section() {
        let p = build_prompt("refactor everything", None, false);
        assert!(p.contains("Task Decomposition"));
        assert!(p.contains("meta.phases") || p.contains("phases"));
    }
}
