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

use crate::core::contract::backend::{AgentBackend, AgentTask, RunContext};
use crate::core::contract::event::AgentEvent;
use crate::runtime::validate_script;
use std::path::PathBuf;
use std::sync::Arc;

/// Planning configuration.
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    /// Model used by the planner agent (`None` = backend default).
    pub planner_model: Option<String>,
    /// Max attempts to (re)generate a valid script before giving up.
    pub max_retries: usize,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            planner_model: None,
            max_retries: 3,
        }
    }
}

/// A planned workflow: the generated Lua orchestration script.
#[derive(Debug, Clone)]
pub struct PlannedWorkflow {
    /// The generated Lua script (validated, fence-stripped).
    pub script: String,
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
        let prompt = build_prompt(task, (attempt > 0).then_some(last_error.as_str()));

        let output = run_planner_agent(&*backend, &prompt, cfg.planner_model.clone())
            .await
            .map_err(PlannerError::Backend)?;

        let script = match extract_script(&output) {
            Some(s) => s,
            None => {
                last_error = "no ```lua code block (or text) found in agent output".to_string();
                continue;
            }
        };

        match validate_generated(&script) {
            Ok(()) => return Ok(PlannedWorkflow { script }),
            Err(e) => last_error = e,
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

/// Validate a generated script: Lua syntax + a required `report(...)` call.
fn validate_generated(script: &str) -> Result<(), String> {
    validate_script(script).map_err(|e| format!("lua syntax error: {}", e))?;
    if !script.contains("report(") {
        return Err("script must call report(...) to emit a final result".to_string());
    }
    Ok(())
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

/// Return the first ```lua (or unlabelled) fenced block; otherwise the trimmed
/// input. No regex dependency — a small hand-rolled fence scanner.
fn extract_lua_block(text: &str) -> String {
    find_fenced_block(text).unwrap_or_else(|| text.trim().to_string())
}

fn find_fenced_block(text: &str) -> Option<String> {
    let mut from = 0;
    while let Some(rel) = text[from..].find("```") {
        let fence = from + rel;
        let after = &text[fence + 3..];
        let line_end = after.find('\n')?;
        let lang = after[..line_end].trim();
        let body_start = fence + 3 + line_end + 1;
        let rest = &text[body_start..];
        let close = rest.find("```")?;
        if lang.eq_ignore_ascii_case("lua") || lang.is_empty() {
            return Some(rest[..close].trim_end().to_string());
        }
        // Not a Lua block — keep scanning after this one.
        from = body_start + close + 3;
    }
    None
}

/// Build the planner prompt: DSL reference + task (+ optional fix-up error).
fn build_prompt(task: &str, fix_error: Option<&str>) -> String {
    let mut p = String::with_capacity(LUA_DSL_REFERENCE.len() + task.len() + 256);
    p.push_str(LUA_DSL_REFERENCE);
    p.push_str("\n\n# Task\n\n");
    p.push_str(task);
    p.push('\n');
    if let Some(err) = fix_error {
        p.push_str("\n# Your previous attempt was rejected\n\n");
        p.push_str(err);
        p.push_str("\n\nFix the script and output a corrected version.\n");
    }
    p.push_str("\nOutput ONLY one ```lua code block — no prose before or after.\n");
    p
}

/// The orchestration DSL spec handed to the planner agent. This is the
/// "target language" the model compiles the task into. Kept in sync with the
/// primitives registered in [`crate::runtime`] (`sandbox.rs`).
const LUA_DSL_REFERENCE: &str = r##"You are the orchestration planner for Maestro, a multi-agent workflow runtime.
Generate a Lua script that orchestrates LLM subagents to accomplish the user's task.

# Execution model
- The Lua script is the ORCHESTRATOR. It holds the loop, branching and intermediate
  results in local variables. Only the final report() value returns to the user.
- The script runs in a SANDBOX: `io`, `os`, `require`, file and shell access are
  DISABLED. The script MUST NOT read files, run commands, or scan directories.
- ALL real work — reading files, grepping, editing, web search, analysis — is done by
  the subagents you spawn. Put those instructions in the agent prompt text; the agent
  has the tools, the script does not.

# Primitives (available as Lua globals)
- agent(opts) -> result
    opts:   { prompt=<string, required>, model=<string?>, schema=<table?>,
              backend=<string?>, timeout_ms=<int?> }
    result: { status=<string>, ok=<bool>, output=<table>, tokens=<int>, findings=<array> }
    Runs ONE subagent to completion.
    - `output` is the agent's response parsed as JSON → Lua table. Access fields
      directly, e.g. `r.output.files`, `r.output.summary`.
    - If `ok` is false, `output` may be nil or an error object; check `status`.
    - `schema` (optional): a JSON Schema (Draft 7) object describing the expected
      output shape. When provided, the runtime validates the agent's output against
      it and rejects mismatches. Express nested types properly:
      Example: schema = { type = "object",
                          properties = { files = { type = "array",
                                                   items = { type = "string" } },
                                         summary = { type = "string" } },
                          required = { "files", "summary" } }

- parallel(items, mapFn) -> array<result>
    items: an array (table). mapFn(item) must RETURN an agent opts table.
    Runs all items concurrently (barrier); results preserve input order.
    Use when you need ALL results before continuing.

- pipeline{ items=<array>, stages={ stageFn1, stageFn2, ... }, max_inflight=<int?> }
      -> { items=<array>, ok=<int>, failed=<int> }
    Streaming / non-barrier: each item flows stage by stage; different items can run
    different stages at the same time. stageFn(data) gets the previous stage's output
    and returns the next. Prefer pipeline() over parallel() by default.

- phase(name, planned?) -> phase_id   -- group work into a progress phase
- log(msg, level?)                     -- emit a status line
- budget(time_ms?, max_rounds?)        -- hint runtime limits
- workflow(path, args?) -> result      -- call another saved workflow as a sub-step
- report(value)                        -- REQUIRED: set the final workflow output
- json.encode(v) / json.decode(s)      -- (de)serialization helpers

# Globals
- args   — table of user-supplied arguments (from --args JSON); access e.g. args.topic.
- ctx    — run context; ctx.run_id is the current workflow run ID (string).

# Error handling
- Always check `result.ok` before using `result.output`.
- On failure, log() the error and decide: skip, retry, or abort early with report().
- Example:
    local r = agent({ prompt = "..." })
    if not r.ok then
      log("agent failed: " .. (r.status or "unknown"), "warn")
      report({ error = r.status })
    end

# Adversarial Verification Pattern (implement in Lua, no SDK call)
When the task needs cross-checked / verified results, implement adversarial
verification directly in Lua using agent() and parallel():
1. PRODUCE: run producer agents (via parallel) on each item to generate findings.
2. CHALLENGE: for each finding, run adversary agents that attempt to refute it.
3. VOTE: keep only findings whose approval rate >= your threshold (e.g. 0.7).
4. ITERATE: feed surviving findings back as items; repeat up to N rounds.
5. STOP when converged (no findings refuted) or max rounds reached.
This is a pattern, not a primitive — write the loop in Lua. Only use it when the
task genuinely requires cross-checking; skip it for simple tasks.

# Rules
1. The script MUST end by calling report(<table>) with the final result.
2. Do NOT touch the filesystem/shell from the script. Tell agents what to do instead.
3. Keep fan-out bounded — at most ~16 concurrent agents. For large or unknown sets,
   have an agent enumerate / chunk the work and return a list you fan out over.
4. Prefer pipeline() for streaming work; parallel() only when you need every result at
   once. For verification / audit / research, implement the adversarial pattern in Lua
   using agent() and parallel() — do NOT call converge().
5. Always check result.ok before using result.output.
6. Use phase() / log() to make progress legible.
7. Output ONLY a single ```lua code block — no explanation.

# Example: simple research workflow
```lua
phase("research", 1)

local topic = args.topic or "AI safety"

-- Step 1: gather sources
local gather = agent({
  prompt = "Research: " .. topic .. ". Return JSON {sources: [{title, url, summary}]}.",
  schema = {
    type = "object",
    properties = {
      sources = { type = "array", items = {
        type = "object",
        properties = { title = { type = "string" }, url = { type = "string" }, summary = { type = "string" } }
      } }
    },
    required = { "sources" }
  }
})
if not gather.ok then
  report({ error = "gather failed: " .. gather.status })
end

-- Step 2: analyze each source in parallel
local results = parallel(gather.output.sources or {}, function(src)
  return { prompt = "Analyze this source and extract key insights.\n" .. json.encode(src) }
end)

report({ topic = topic, sources = #results, results = results })
```

# Example: adversarial verification snippet (add when cross-checking is needed)
```lua
-- Multi-round adversarial loop (skeleton)
local items = gather.output.findings or {}
local max_rounds = 3
local threshold = 0.7

for round = 1, max_rounds do
  log("adversarial round " .. round)
  local votes = parallel(items, function(finding)
    return { prompt = "Evaluate this finding for accuracy. Return JSON {approve: true|false}.\n"
                   .. json.encode(finding) }
  end)
  local survivors = {}
  for i, finding in ipairs(items) do
    if votes[i].ok and votes[i].output.approve then
      table.insert(survivors, finding)
    end
  end
  if #survivors == #items then break end
  items = survivors
end
```
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{MockBackend, MockBehavior, TokenUsage};
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
        let script = "```lua\nlocal r = agent({prompt='hi'})\nreport({ok=true})\n```";
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
}
