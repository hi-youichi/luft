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
use crate::runtime::{validate_script, validate_workflow};
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
        if attempt > 0 {
            tracing::warn!(
                attempt,
                total = attempts,
                "retrying script generation after validation failure"
            );
        }

        let prompt = build_prompt(task, (attempt > 0).then_some(last_error.as_str()));

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
            Ok(()) => return Ok(PlannedWorkflow { script }),
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
fn build_prompt(task: &str, fix_error: Option<&str>) -> String {
    let mut p = String::with_capacity(LUA_DSL_REFERENCE.len() + task.len() + 512);
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
/// Every primitive documented here MUST be registered in [`crate::runtime::sandbox`]
/// via `register_sdk()`. If you add a new Lua global, document it here and register
/// it there. If you remove one, remove it from both places.
///
/// ## Token cost
///
/// This reference is sent on every planner call (~4K tokens). Keep examples
/// concise but illustrative. Rules are authoritative — primitives are reference.
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

# Workflow Architecture Comment
Every script MUST begin with a header comment whose Arch section is a
multi-line ASCII architecture diagram (boxes + arrows). This forces
plan-then-code thinking and makes the workflow readable at a glance.
Format:

--------------------------------------------
-- Goal:  <one-line objective, English>
-- Arch:
--   <multi-line ASCII box-and-arrow diagram>
-- Flow:  <single-line data flow chain>
--------------------------------------------

Header rules:
- Two delimiter lines of 44 dashes wrapping the block.
- Goal: a single English line stating what the workflow produces.
- Arch: a multi-line ASCII box-and-arrow diagram (see # Diagram Grammar).
  Every line of the diagram is prefixed with `-- ` so the whole block is a
  valid Lua comment. Prefer detail over brevity: draw every phase, branch
  and artifact so a reader can trace the whole workflow from the diagram.
- Flow: a single line showing the global data flow as a chain of
  artifacts (e.g., discover -> subsystems[] -> modules[] -> report).
- This comment goes at the VERY TOP, before any schema locals or code.
- If the task is decomposed (see # Task Decomposition), the diagram MUST
  show the decomposition as a `=====>` fan-out annotated with
  `(for each X)` and a matching `<=====` fan-in — one fan-out level per
  decomposition dimension.

# Diagram Grammar

Boxes represent phases / steps. Draw them as multi-line rectangles:
    +----------+
    | name     |
    +----------+
- Box name: short phase label (lowercase; match meta.phases[].label when
  practical). One word or short phrase. Pad the name line with trailing
  spaces so the right border `|` lines up with the `+` borders above and
  below it.
- Keep box widths uniform within one diagram where possible; pad narrow
  boxes with spaces so their right borders align vertically. Mismatched
  widths are ugly but not an error — alignment of borders within a single
  box MUST be correct.

Arrows represent data / control flow between boxes:
    ------>        sequential flow (one step finishes, next starts)
    =====>         fan-out: spawn multiple parallel branches
    <=====         fan-in / join: converge branches back into one
    --> [name]     artifact produced by a step (suffix on a box or arrow)

Inline annotations (attach to an arrow, or write above a branch group):
    (for each X)      loop / decomposition dimension (X = module, file, ...)
    (retry <= N)      bounded retry loop around a box or sub-chain
    (degrade on fail) error strategy: fall back instead of fail
    (parallel)        this branch group runs concurrently
    (pipeline)        this branch group runs as staged pipeline

Layout rules:
- Read top-to-bottom, then left-to-right.
- Sequential chains stack vertically (one box per line), or run
  left-to-right on one line when they fit.
- Fan-out: a single `====>` splits into N branch boxes; each branch may
  produce an artifact via `--> [name]`.
- Fan-in: a single `<====` MUST converge ALL open branches before the next
  sequential box. A branch that never joins back is a diagram error.
- Indent nested fan-out groups 2 extra spaces per nesting level.
- Artifacts (`--> [name]`) hang off the right side of the box or arrow
  that produces them; the Flow line then references these [name]s.

Examples (every line carries the `-- ` comment prefix in real output):

(1) Linear workflow:
--   +----------+        +---------+        +--------+
--   | discover |------->| analyze |------->| report |
--   +----------+        +---------+        +--------+
--        |                  |
--        v                  v
--   --> [targets]      --> [findings]

(2) Parallel fan-out / fan-in:
--                        +-------+
--                 ======>| fetch |=====> [sources]
--                 |      +-------+
--                 |      +-------+
--   +------+      |=====>| parse |=====> [docs]
--   | plan |======|      +-------+
--   +------+      |      +-------+
--                 |=====>| index |=====> [chunks]
--                 |      +-------+
--                 |
--                 |      (parallel)
--                 v
--                 +-------+        +--------+
--                 | merge |------->| report |
--                 +-------+        +--------+

(3) Decomposed per-module mini-workflow with retry:
--   +----------+
--   | discover |=====> (for each module)
--   +----------+        |
--        |              +--------+        +--------+        +--------+
--        |              | analyze|=======>| change |=======>| verify |--> [result]
--        |              +--------+        +--------+        +--------+
--        |              (retry <= 2)               (degrade on fail)
--        v
--   +--------+
--   | report |<-----[results[]]
--   +--------+

# Meta Table & Entry Point
Every script MUST declare a `meta` table and a `function main()` entry point.
The meta table is extracted before execution to render a plan preview in the CLI.

Format:
```lua
meta = {
  reasoning = "<one-line explanation of the workflow strategy>",
  phases = {
    {
      label = "<phase name>",
      description = "<one-line description shown in CLI>",
      agents = <int>,                  -- planned agent count (for progress display)
      depends_on = { <int>, ... },     -- indices of phases that must complete first
      dynamic = false,                 -- true for phases inside loops/parallel/pipeline
    },
  },
}
```

Field reference (`phases[i]`):
- `label` (string, REQUIRED) — phase name (shown in CLI).
- `description` (string, optional but recommended) — one-line description
  shown in CLI so the user understands what this phase does. Keep it short.
- `agents` (int, optional) — planned agent count, e.g. parallel fan-out
  size, used for progress display. Hint only, not enforced.
- `depends_on` (int[], optional) — indices of earlier phases that must
  complete before this phase starts. Empty `{}` or omitted = no dependency.
- `dynamic` (bool, default false) — true for phases whose items are
  discovered at runtime (e.g. inside a `for each` loop over agent results).

Rules:
- `meta` MUST be the first statement after the header comment.
- `meta.reasoning` — a single English line explaining the approach.
- `meta.phases` — an array of `{ label, description?, agents?,
  depends_on?, dynamic? }` entries listing the top-level phases. This is a
  static preview; not every runtime `phase()` call needs to be listed —
  only the main structural phases.
- `description`, `agents`, and `depends_on` are RECOMMENDED for every
  phase that has a non-trivial structure; they make the CLI plan preview
  readable and let the user understand ordering and parallelism at a
  glance. Only `label` is strictly required.
- `dynamic` defaults to false. Set it to true for phases whose items are
  discovered at runtime (e.g., inside a `for each` loop over agent results).
- After `meta`, declare any schema locals, then define `function main()`.
- ALL execution code (agent calls, phase calls, loops, report) goes inside
  `main()`. The top level must only contain meta, locals, and function defs.

Minimal skeleton:
```lua
--------------------------------------------
-- Goal:  <one-line objective>
-- Arch:
--   +---------+      +---------+      +--------+
--   | discover|----->| process |----->| report |
--   +---------+      +---------+      +--------+
-- Flow:  discover -> items[] -> results -> report
--------------------------------------------
meta = {
  reasoning = "...",
  phases = {
    { label = "discover" },
    { label = "process", dynamic = true },
    { label = "report" },
  },
}
local SCHEMA = { ... }
function main()
  phase("discover")
  ...
  report({ result = ... })
end
```

# Task Decomposition
Break large tasks into smaller, independent units of work. Each unit becomes a
phase span; inside each span runs a similar mini-workflow (e.g., analyze ->
change -> verify).

When to decompose:
- The task touches multiple files, modules, subsystems, or documents.
- The task has multiple distinct phases that could be described separately
  (e.g., "find issues, then fix them, then verify").
- The scope is unknown or large — first spawn an agent to enumerate targets,
  then loop over the returned list with one span per target.
- NOT needed for single-file, single-step tasks (a linear script is fine).

Granularity:
- One span = one work unit (one module / file / subsystem / document).
- Inside a span: a fixed mini-workflow of 2-4 agent phases
  (e.g., analyze -> change -> verify). Reuse the same phase sequence in every
  span so the workflow is uniform and predictable.
- Do NOT cram everything into a single agent call with a huge prompt.
- Do NOT over-split into one-agent spans with no internal phases.

Decomposition dimension (pick one, matching the task):
- by file/module   — code changes, refactoring
- by subsystem     — audits, cross-cutting reviews
- by document      — documentation work
- by finding/item  — verification, research, triage

Anti-patterns (do NOT do these):
- One giant agent() call that "does everything" — impossible to verify,
  impossible to resume, prompt is unmanageably long.
- Hardcoding a list of targets (e.g., module names) when the task does not
  specify them — enumerate with an agent first.
- Mixing decomposition dimensions (e.g., some spans per-file, others
  per-subsystem) in the same workflow — pick one dimension and stay consistent.
- A span whose mini-workflow has only one agent call with no phases —
  that is not a unit of work, it is a step; use phase() instead.

# Primitives (available as Lua globals)

## agent(opts) -> result
    Runs ONE subagent to completion. This is the fundamental work unit.
    opts:   { prompt=<string, required>,     -- instructions for the subagent
              schema=<table?>,               -- JSON Schema to constrain output (RECOMMENDED)
              model=<string?>,               -- override model (default: backend's model)
              name=<string?>,                -- short agent identifier shown in CLI (e.g. "analyze-auth")
              description=<string?>,         -- one-line description shown in CLI (e.g. "审查 auth 模块安全")
              timeout_ms=<int?> }            -- per-agent timeout
    result: { ok=<bool>,                     -- true if agent succeeded
              status=<string>,               -- "ok" / "error" / "cancelled" / "timed_out"
              output=<table>,                -- agent response (parsed JSON → Lua table)
              tokens=<int>,                  -- token usage
              findings=<array> }             -- accumulated findings (if any)

    MUST-DO pattern:
      local r = agent({ prompt = "...", schema = MY_SCHEMA })
      if not r.ok then
        log("agent failed: " .. (r.status or "unknown"), "warn")
        -- decide: skip, retry, or abort with report()
      end
      local data = r.output   -- safe to access when schema was provided

    schema (STRONGLY RECOMMENDED):
      A JSON Schema (Draft 7) object. When provided, the runtime forces structured
      output, validates it, and retries on mismatch. WITHOUT a schema, output is
      free-form text that may not be valid JSON — accessing `r.output.xxx` on
      malformed output silently yields nil and breaks downstream code.
      ALWAYS provide a schema when you access specific fields in r.output.
      Define named schema tables at the top of the script and reuse them:
        local FINDINGS = {
          type = "object",
          properties = {
            files = { type = "array",
                      items = { type = "object",
                                properties = { path = { type = "string" },
                                               purpose = { type = "string" } },
                                required = { "path", "purpose" } } },
            summary = { type = "string" }
          },
          required = { "files", "summary" }
        }
      Then: agent({ prompt = "...", schema = FINDINGS })

## parallel(items, mapFn) -> array<result>
    Barrier fan-out: runs all items concurrently, waits for ALL to finish.
    items:  array of work items (any Lua table).
    mapFn:  function(item) → must RETURN an agent opts table (same shape as agent()).
    Result: array of agent results, preserving input order.
    Use when: you need ALL results before continuing (e.g. gather → analyze all).

    Example:
      local results = parallel(urls, function(url)
        return { prompt = "Fetch and summarize: " .. url, schema = SUMMARY }
      end)

## pipeline{ items=, stages=, max_inflight= } -> { items=, ok=, failed= }
    Streaming multi-stage: each item flows through all stages; different items can
    be in different stages simultaneously. Prefer pipeline() over parallel() by default.

    IMPORTANT: Unlike parallel(), pipeline stage handlers are NOT auto-executed. Each
    handler MUST call agent() itself and return the result (or custom data). The return
    value becomes the input to the next stage.

    Parameters:
      items:       array of work items.
      stages:      array of stages. Each stage is either a function(prev) or a
                   table { label=, handler=function(prev) }. The handler receives
                   the previous stage's return value (or the raw item for stage 1),
                   calls agent() internally, and returns its result.
      max_inflight: max concurrent items (default: 4).

    Stage data flow:
      Stage 1:  handler(item)     → [calls agent()] → return value(data₁)
      Stage 2:  handler(data₁)    → [calls agent()] → return value(data₂)
      Stage 3:  handler(data₂)    → [calls agent()] → return value(data₃)
      ...
      pipeline_result.items[i].output is the LAST stage's return value for item i.
      pipeline_result.items[i].stages[j] = { label, status, elapsed_ms }.

    Error degradation:
      If a stage returns a failed result (prev.ok = false), the next stage still
      receives it. Check `prev.ok` at the start of each handler and decide: degrade
      gracefully or abort. On degrade, return default data directly (do NOT call agent).
      Example:
        function(prev)
          if not prev.ok then
            return { ok = false, output = { module = "unknown", score = 0 } }
          end
          return agent({ prompt = "Process: " .. json.encode(prev.output), schema = SCHEMA })
        end

    Example (2-stage: analyze → assess):
      local results = pipeline{
        items = modules,
        max_inflight = 4,
        stages = {
          function(mod)
            phase("analyze " .. mod.name)
            return agent({ prompt = "Analyze " .. mod.path, schema = ANALYSIS })
          end,
          function(prev)
            phase("assess " .. (prev.output and prev.output.module or "?"))
            if not prev.ok then
              return { ok = false, output = { module = "unknown", score = 0 } }
            end
            return agent({ prompt = "Assess: " .. json.encode(prev.output), schema = ASSESS })
          end
        }
      }

## phase(name, planned?) -> phase_id
    Declares a progress phase. Emits a PhaseStarted event visible in CLI output.
    Use phase() for individual steps inside a larger span or for flat workflows.
    name:    human-readable label (shown in CLI phase tree).
    planned: expected agent count (optional, for progress display).

    Example:
      phase("analyze", 1)
      local r = agent({ prompt = "...", schema = S })

## phase_begin(name, planned?) -> span_id
## phase_end(span_id?)
    Opens / closes a STRUCTURAL phase span. Unlike phase(), spans can nest and
    support resume. Always pair them: every phase_begin() MUST have a phase_end().
    Use for: per-unit work in a loop (e.g. "review <module>").

    Nesting guidance:
      2 levels (default):  outer span + inner phase() steps
        phase_begin("review module-A")
          phase("analyze")  → phase("report")
        phase_end()
      3 levels (large scope):  group span + unit span + inner steps
        phase_begin("review subsystem")
          phase_begin("review module-A")
            phase("analyze") → phase("assess")
          phase_end()
          phase_begin("review module-B")
            phase("analyze") → phase("assess")
          phase_end()
        phase_end()

## log(msg, level?)
    Emits a status line visible in CLI output and event log.
    level: "info" (default) / "warn" / "error".

## budget(time_ms?, max_rounds?)
    Hints resource limits for the current phase. Optional.
    time_ms:   total wall-clock budget in milliseconds.
    max_rounds: max agent conversation rounds.

## workflow(path, args?) -> result
    Calls another saved workflow as a sub-step.
    path: relative path to the .lua workflow file.
    args: table of arguments passed to the sub-workflow.

## report(value)
    REQUIRED: sets the final workflow output and ends the run.
    Call exactly ONCE — the first call wins; later calls are ignored.
    Always `return` after an error report() to prevent fall-through.

## json.encode(value) / json.decode(string)
    JSON serialization helpers for passing structured data to/from agent prompts.

# Globals
- args             — table of user-supplied arguments (from --args JSON); access e.g. args.topic.
- ctx              — run context; ctx.run_id is the current workflow run ID (string).
- completed_spans  — when non-nil (resume mode), a table whose keys are completed span names.
                     Check before phase_begin() to skip already-done work. See Resume section.

# Resume Mode
When the runtime resumes a previously interrupted run, the `completed_spans` global
is non-nil. It is a table whose keys are the names of spans that already completed.
Check it before every phase_begin() call and skip matching spans via `goto continue`.

Resume skip pattern (use this exact idiom at the top of every loop body):
```lua
for _, item in ipairs(items) do
  local name = "review " .. item.name
  if completed_spans and completed_spans[name] then
    log("skipping completed: " .. name)
    goto continue
  end
  local span = phase_begin(name)
    -- ... work ...
  phase_end(span)
  ::continue::
end
```

# Error Handling
- ALWAYS check `result.ok` before using `result.output`.
- On failure: log() the error, then decide — skip, retry, or abort with report().
- Always `return` after an error report() to prevent nil dereference.
- Graceful degradation: when a stage fails, feed a minimal/default prompt to the
  next stage rather than crashing the pipeline.

  Example:
    local r = agent({ prompt = "...", schema = S })
    if not r.ok then
      log("agent failed: " .. (r.status or "unknown"), "warn")
      report({ error = r.status })
      return
    end

# Adversarial Verification Pattern (implement in Lua)
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
1. The script MUST begin with a workflow architecture comment header (see
   # Workflow Architecture Comment). No code or schema locals before it.
2. The script MUST end by calling report(<table>) with the final result.
3. Do NOT touch the filesystem/shell from the script. Tell agents what to do instead.
4. Keep fan-out bounded — at most ~16 concurrent agents. For large or unknown sets,
   have an agent enumerate / chunk the work and return a list you fan out over.
5. Prefer pipeline() for streaming work; parallel() only when you need every result at
   once. For verification / audit / research, implement the adversarial pattern in Lua
   using agent() and parallel().
6. ALWAYS check result.ok before using result.output.
7. ALWAYS provide a `schema` for every agent() / parallel() / pipeline() call whose
   result output you access by field name (e.g. `r.output.files`). Without a schema,
   output is unvalidated free-form text and field access will silently yield nil.
   Define schema tables as locals at the top of the script and reuse them.
8. Always `return` after an error report() so execution does not fall through to
   code that dereferences nil.
9. Call report() exactly ONCE — the first call wins; later calls are ignored.
10. Use phase() / log() to make progress legible.
11. Output ONLY a single ```lua code block — no explanation.
12. ALWAYS enclose string values in double quotes — especially non-ASCII text
   (Chinese, Japanese, etc.). Lua identifiers are ASCII-only; bare CJK characters
   outside a quoted string are a syntax error. Write `prompt = "整理文档"`, NEVER
   `prompt = 整理文档`. This applies to table fields, function arguments, and
   string concatenation operands alike.
13. For large tasks (refactoring multiple modules, auditing multiple subsystems),
    decompose into phase spans. Each span wraps a similar internal workflow
    (e.g., analyze → change → verify). Put phase_begin()/phase_end() around
    each unit; use phase() for steps inside.
14. For unknown scopes, have an agent enumerate targets first, then loop with
    phase_begin() per target. Do NOT hardcode module names unless the task
    specifies them.
15. ALWAYS pair phase_begin() with phase_end(). Unpaired phase_begin() is a
    runtime error.
16. Spans can nest (2-3 levels). Use 2 levels by default (span + steps);
    use 3 levels (group span + module span + steps) for whole-crate/monorepo tasks.
17. When resuming (the `completed_spans` global is non-nil), skip spans whose
    name matches an entry. Use `goto continue` to skip.

# Example: per-module refactoring (static decomposition)
```lua
--------------------------------------------
-- Goal:  Refactor auth, db, api modules
-- Arch:
--   +---------+        +----------+        +--------+
--   | analyze |=======>| refactor |=======>| verify |--> [VERIFY]
--   +---------+        +----------+        +--------+
--   (for each module in {auth, db, api})
-- Flow:  {modules} -> ANALYSIS -> CHANGES -> VERIFY -> report
--------------------------------------------
local MODULES = { "auth", "db", "api" }
local results = {}

for _, mod in ipairs(MODULES) do
  local name = "refactor " .. mod
  if completed_spans and completed_spans[name] then
    log("skipping completed: " .. name)
    goto continue
  end
  local m = phase_begin(name)
    phase("analyze")
    local a = agent({ prompt = "Analyze " .. mod .. " for issues", schema = ANALYSIS })

    phase("refactor")
    local c = agent({ prompt = "Apply refactoring to " .. mod, schema = CHANGES })

    phase("verify")
    local v = agent({ prompt = "Verify " .. mod .. " still passes tests", schema = VERIFY })
    table.insert(results, { module = mod, ok = v.ok })
  phase_end(m)
  ::continue::
end

report({ refactored = #results, results = results })
```

# Example: whole-crate refactoring (dynamic enumeration, 3-level nesting)
```lua
--------------------------------------------
-- Goal:  Refactor entire crate by subsystem
-- Arch:
--   +----------+        +----------+        +---------+        +--------+        +--------+
--   | discover |=======>| discover |=======>| analyze |=======>| change |=======>| verify |
--   +----------+        +----------+        +---------+        +--------+        +--------+
--    subsystems          modules           (for each subsystem, then each module)
-- Flow:  discover -> subsystems[] -> modules[] -> changes -> report
--------------------------------------------
phase("discover subsystems")
local discover = agent({
  prompt = "Enumerate subsystems under src/ that need refactoring",
  schema = SUBSYSTEMS_SCHEMA
})

for _, sys in ipairs(discover.output.subsystems or {}) do
  local gname = "refactor " .. sys.name
  if completed_spans and completed_spans[gname] then
    goto skip_sys
  end
  local g = phase_begin(gname)
    local mods = agent({
      prompt = "List modules in " .. sys.path .. " needing changes",
      schema = MODULES_SCHEMA
    })
    for _, mod in ipairs(mods.output.modules or {}) do
      local mname = "refactor " .. mod.name
      if completed_spans and completed_spans[mname] then
        goto skip_mod
      end
      local m = phase_begin(mname)
        phase("analyze")
        phase("change")
        phase("verify")
      phase_end(m)
      ::skip_mod::
    end
  phase_end(g)
  ::skip_sys::
end

report({ done = true })
```

# Example: simple research workflow
```lua
--------------------------------------------
-- Goal:  Research a topic and analyze sources
-- Arch:
--   +--------+        +---------+
--   | gather |=======>| analyze |--> [ANALYSIS[]]
--   +--------+        +---------+
--                (parallel, for each source)
-- Flow:  gather -> sources[] -> parallel(analyze) -> report
--------------------------------------------
phase("research", 1)

local topic = args.topic or "AI safety"

local SOURCES_SCHEMA = {
  type = "object",
  properties = {
    sources = { type = "array", items = {
      type = "object",
      properties = { title = { type = "string" }, url = { type = "string" }, summary = { type = "string" } },
      required = { "title", "summary" }
    } }
  },
  required = { "sources" }
}

local ANALYSIS_SCHEMA = {
  type = "object",
  properties = {
    insights = { type = "array", items = { type = "string" } },
    credibility = { type = "string" }
  },
  required = { "insights" }
}

local gather = agent({
  prompt = "Research: " .. topic,
  schema = SOURCES_SCHEMA
})
if not gather.ok then
  report({ error = "gather failed: " .. gather.status })
  return
end

local results = parallel(gather.output.sources or {}, function(src)
  return {
    prompt = "Analyze this source and extract key insights.\n" .. json.encode(src),
    schema = ANALYSIS_SCHEMA
  }
end)

report({ topic = topic, sources = #results, results = results })
```

# Example: adversarial verification snippet (add when cross-checking is needed)
```lua
--------------------------------------------
-- Goal:  Cross-check findings via voting
-- Arch:
--   +------+        +--------+
--   | vote |=======>| keep   |--> [survivors[]]
--   +------+        +--------+
--      ^                 |
--      +<================+   (repeat <= N rounds; break if converged)
-- Flow:  findings -> vote -> survivors -> (loop) -> report
--------------------------------------------
-- Multi-round adversarial loop (skeleton)
local items = gather.output.findings or {}
local max_rounds = 3
local threshold = 0.7

local VOTE_SCHEMA = {
  type = "object",
  properties = { approve = { type = "boolean" } },
  required = { "approve" }
}

for round = 1, max_rounds do
  log("adversarial round " .. round)
  local votes = parallel(items, function(finding)
    return {
      prompt = "Evaluate this finding for accuracy.\n" .. json.encode(finding),
      schema = VOTE_SCHEMA
    }
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
    use crate::core::{FailKind, MockBackend, MockBehavior, TokenUsage};
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
        let p = build_prompt("do task", Some("script had syntax error"));
        assert!(p.contains("do task"));
        assert!(p.contains("previous attempt was rejected"));
        assert!(p.contains("script had syntax error"));
        assert!(p.contains("Output ONLY"));
    }

    #[test]
    fn test_build_prompt_without_fix_error() {
        let p = build_prompt("do task", None);
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
        let p = build_prompt("refactor everything", None);
        assert!(p.contains("Task Decomposition"));
        assert!(p.contains("phase_begin"));
    }
}
