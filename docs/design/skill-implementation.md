# Luft Skill — Implementation Plan

> **Status**: §1–§4 implemented and tested. §4.0's open question was resolved: write per-backend, keyed off which backend is actually being spawned (`config.id`) — not gated on task content (options (b)/(c) were not chosen). Companion to [`skill-architecture.md`](./skill-architecture.md) (the "why"); this doc is the "what, file by file, in what order."

---

## 0. Decisions taken on the open questions

The design doc left 5 open questions. Concrete calls for this implementation, so the plan below has no forks:

| # | Question | Decision | Why |
|---|------|------|-----|
| 1 | Split granularity | See §1 exact mapping | Mechanical, derived from the existing 688-line file's own section boundaries |
| 2 | Clean up runtime-written skill files after a run? | **No.** Leave them. | The run's working folder is already scratch space; next run overwrites the same paths. Adding cleanup is extra failure-mode surface (what if cleanup fails? what if two runs share a working folder concurrently?) for no real benefit |
| 3 | MCP channel (`workflow://reference/{name}`) this round? | **No — out of scope for this pass.** | Needs new `luft-mcp` resource-template code; not blocking the other two channels |
| 4 | Fold `luft-mcp`'s vendored copy into this? | **Yes.** | It's the same duplication problem this whole effort exists to fix; doing it now avoids a second review pass touching the same files |
| 5 | Add Claude Code as a spawnable backend? | **No — out of scope.** | Adding a new ACP backend is unrelated scope |
| 6 | Write skill files to all three target dirs unconditionally, or only the one matching the spawned backend? | **Superseded — see §4.2.** Only the matching backend's directory. | `AcpConfig.id` is already available where this logic runs; writing an agent's own convention only is more correct than writing all three and hoping the wrong ones are harmless |
| 7 | Shared abstraction for MCP-server injection and skill-file injection (both "prepare something before `session/new`, given backend id + working folder")? | **No `trait`/registry.** Two parallel functions sharing one `SessionSetupCtx` struct. | Only two concrete "assets" exist today and no third is planned (the MCP reference channel is explicitly out of scope, row 3). A `dyn SessionAsset` registry would be premature abstraction for N=2; revisit if a third asset shows up |

---

## 1. Step 1 — Split `lua_dsl_reference.md`

### 1.1 Exact mapping (source line ranges → destination)

Source: [`crates/luft-planner/src/lua_dsl_reference.md`](../../crates/luft-planner/src/lua_dsl_reference.md) (688 lines, current).

| Source lines | Section | Destination |
|---|---|---|
| 1 | Intro sentence | Main body — **fix "Maestro" → "Luft" while moving** |
| 4–6 | `# Output Format` | Main body |
| 8–15 | `# Execution Model` | Main body |
| 17–69 | `# Architecture Header` (diagram notation) | `references/architecture-header.md` |
| 71–102 | `# Meta Table & Entry Point` | Main body |
| 104–126 | Minimal skeleton | Main body |
| 128–155 | `# Agent Prompt Quality` | `references/agent-prompts.md` |
| 157–189 | `# Task Decomposition` | `references/task-decomposition.md` |
| 190–310 | `# Primitives` (full signatures + examples for agent/parallel/pipeline/phase/log/budget/workflow/report/json) | `references/primitives.md` — main body gets a one-line-per-primitive index instead (name + one-line purpose + "full signature: references/primitives.md") |
| 311–314 | `# Globals` | Main body |
| 315–321 | `# Error Handling` | Main body |
| 322–336 | `# Adversarial Verification Pattern` | `references/adversarial-verification.md` |
| 337–365 | `# Rules` | Main body (kept — these are must-follow, not optional depth) |
| 366–453 | Example: per-module refactoring | `references/examples.md` |
| 455–592 | Example: whole-crate refactoring | `references/examples.md` |
| 594–688 | Example: adversarial verification | `references/examples.md` |

Result: main body ≈ 150–200 lines (comparable to loom's 140-line `workflow_skill.md`); 6 reference files totaling the rest, byte-for-byte extracted (no rewording in this pass — see §0 row 1's "mechanical" framing and the architecture doc's §3 constraint).

### 1.2 New file layout

```
crates/luft-planner/src/
├── lua_dsl_reference.md          (deleted — replaced by skill/ below)
└── skill/
    ├── main.md
    └── references/
        ├── architecture-header.md
        ├── primitives.md
        ├── agent-prompts.md
        ├── task-decomposition.md
        ├── adversarial-verification.md
        └── examples.md
```

### 1.3 Code changes — `crates/luft-planner/src/lib.rs`

```rust
const SKILL_MAIN: &str = include_str!("skill/main.md");
const REF_ARCHITECTURE_HEADER: &str = include_str!("skill/references/architecture-header.md");
const REF_PRIMITIVES: &str = include_str!("skill/references/primitives.md");
const REF_AGENT_PROMPTS: &str = include_str!("skill/references/agent-prompts.md");
const REF_TASK_DECOMPOSITION: &str = include_str!("skill/references/task-decomposition.md");
const REF_ADVERSARIAL_VERIFICATION: &str = include_str!("skill/references/adversarial-verification.md");
const REF_EXAMPLES: &str = include_str!("skill/references/examples.md");

/// Full reference, reassembled from the split files in the same order as the
/// original monolithic `lua_dsl_reference.md`. This is what the planner's
/// system prompt uses — splitting the file must not change planner behavior.
pub const LUA_DSL_REFERENCE: &str = const_str_concat!(
    SKILL_MAIN, "\n",
    REF_ARCHITECTURE_HEADER, "\n",
    REF_PRIMITIVES, "\n",
    REF_AGENT_PROMPTS, "\n",
    REF_TASK_DECOMPOSITION, "\n",
    REF_ADVERSARIAL_VERIFICATION, "\n",
    REF_EXAMPLES,
);

pub const WORKFLOW_SKILL: luft_core::Skill = luft_core::Skill {
    name: "workflow",
    description: "Lua DSL reference for writing multi-agent Luft workflows",
    content: SKILL_MAIN,
    references: &[
        ("references/architecture-header.md", REF_ARCHITECTURE_HEADER),
        ("references/primitives.md", REF_PRIMITIVES),
        ("references/agent-prompts.md", REF_AGENT_PROMPTS),
        ("references/task-decomposition.md", REF_TASK_DECOMPOSITION),
        ("references/adversarial-verification.md", REF_ADVERSARIAL_VERIFICATION),
        ("references/examples.md", REF_EXAMPLES),
    ],
};
```

`LUA_DSL_REFERENCE` can no longer be a plain `const &str` literal concatenation via `+` (not allowed in const context for `&str`) — needs either the `const_format` crate's `concatcp!`/`formatcp!` macro (add as a dependency), or a `once_cell`/`std::sync::LazyLock<String>` built at first use instead of a `const`. Recommend **`const_format::concatcp!`** to keep it a true compile-time `const` (matches the existing all-`&'static str`, zero-alloc design) — add `const_format = "0.2"` to `luft-planner/Cargo.toml`.

### 1.4 Reconciliation test

```rust
// crates/luft-planner/src/lib.rs, in mod tests
#[test]
fn split_content_reassembles_to_the_full_reference() {
    // Every section that existed in the pre-split monolithic file must be
    // present somewhere in the reassembled LUA_DSL_REFERENCE. Guards against
    // silently dropping a section during the split.
    for marker in [
        "# Output Format", "# Execution Model", "# Architecture Header",
        "# Meta Table & Entry Point", "# Agent Prompt Quality",
        "# Task Decomposition", "# Primitives", "# Globals",
        "# Error Handling", "# Adversarial Verification Pattern", "# Rules",
        "# Example: per-module refactoring",
        "# Example: whole-crate refactoring",
        "# Example: adversarial verification",
    ] {
        assert!(
            LUA_DSL_REFERENCE.contains(marker),
            "missing section after split: {marker}"
        );
    }
    assert!(!LUA_DSL_REFERENCE.contains("Maestro"), "stale project name survived the split");
}
```

---

## 2. Step 2 — Consolidate `luft-mcp`'s vendored copy

### 2.1 Delete

`crates/luft-mcp/src/lua_dsl_reference.md` (the vendored copy).

### 2.2 Edit `crates/luft-mcp/src/resources.rs`

```diff
-/// Vendored copy of `luft-planner/src/lua_dsl_reference.md`, embedded at
-/// compile time. Kept as a per-crate copy because `include_str!` cannot
-/// reach outside the published crate's source tree, and re-exporting
-/// through `luft-planner` would force a yank+republish cycle on every
-/// DSL change. Keep this file in sync with the planner's source.
-pub const SCHEMA_MARKDOWN: &str = include_str!("lua_dsl_reference.md");
+/// Re-exported from `luft_planner::LUA_DSL_REFERENCE` — no local vendored
+/// copy. `luft-mcp` already depends on `luft-planner`, so there is no
+/// cross-crate `include_str!` problem to work around; the prior vendoring
+/// only existed to avoid that (nonexistent, in this direction) constraint.
+pub const SCHEMA_MARKDOWN: &str = luft_planner::LUA_DSL_REFERENCE;
```

This closes the exact gap `skill-architecture.md` §1 point 3 called out — the doc comment claiming "the two stay in sync by construction" becomes true instead of aspirational.

### 2.3 Verify

`crates/luft-mcp/Cargo.toml` already lists `luft-planner` as a dependency (confirmed — it's used for `luft_runtime::validate_workflow` already in `tools.rs`), so no new dependency edge.

---

## 3. Step 3 — Delete the dead file

`crates/luft-cli/src/lua_dsl_reference.md` — confirmed zero `include_str!` references anywhere in the codebase. Delete outright.

---

## 4. Step 4 — Runtime channel: write skill files before spawning an ACP backend

### 4.0 Resolved: write per-backend, unconditional on every spawn

Decision: **option (a)**, refined — write unconditionally on every `agent()` spawn, but only into the directory matching the backend actually being spawned (never all three). The "most agent() calls have nothing to do with authoring workflows" clutter concern (below) was accepted as a real but tolerable cost — an unused `SKILL.md` sitting in a sub-agent's working folder is inert; the agent has no reason to read it unless its task happens to need it, and the alternative (task-content gating, option (b)) would have required new undesigned `AgentTask`/Lua-facing API surface for a benefit that doesn't clearly outweigh that cost. Kept as the original discussion for context:

Both `prepare_schema_mcp` and the new skill-writing step run once per `run_acp_session` call — i.e. once per **every** `agent()` spawn in a workflow, not just spawns where the sub-agent's task is about authoring/debugging a Luft workflow. Most `agent()` calls in a real workflow (file analysis, code edits, review) have **no reason** to ever read a "how to write Luft workflows" skill — writing it unconditionally on every spawn is at best inert clutter in the sub-agent's working folder, at worst a false signal ("why is there a workflow-authoring skill available for a task about reviewing a Rust module?").

The one path that unambiguously needs this knowledge — `luft-planner`'s own NL→Lua generation (`backend.run(...)`, see `luft-planner/src/lib.rs`'s module doc) — **already gets the full `LUA_DSL_REFERENCE` inlined directly into its prompt**, independent of any skill-discovery mechanism. So the runtime channel's actual target audience is narrower than "every spawned agent": it's specifically sub-agents whose `AgentTask` prompt is itself about writing/debugging a nested Luft workflow (e.g. via the `workflow(path, args?)` primitive, or a workflow that asks an agent to draft another workflow).

**This is not resolved.** Options on the table, not yet chosen:
- (a) Write unconditionally on every spawn anyway (simplest, accept the clutter/false-signal cost)
- (b) Gate on some signal in `AgentTask` (e.g. a prompt keyword heuristic, or a new explicit flag the calling Lua script sets) — adds a real API surface question (`agent({ ..., needs_workflow_skill = true })`?) that hasn't been designed
- (c) Drop the runtime channel entirely for now; rely on the library channel (loom's own tool-workflow crate, already solved) and revisit runtime-installed skills only if a concrete need shows up

### 4.1 Where (implemented)

`crates/luft-adapters/src/acp_adapter.rs`, right after the shared `SessionState` is assembled and before `prepare_schema_mcp` (same phase, see its doc comment "Phase 3: schema MCP"). §0 row 7's "no `trait`, share a struct" call turned out not to need even a shared struct in practice — `write_workflow_skill_files` only needs `backend_id`/`working_folder` (not `output_schema`, which is `prepare_schema_mcp`'s alone), so it just takes those two as plain parameters:

```rust
write_workflow_skill_files(config.id, &state.cwd);
```

`state.cwd` (already computed by canonicalizing `task.workdir`) is the working folder; `config.id` is the backend identity — both were already in scope at the call site, no new plumbing needed.

### 4.2 Functions (implemented, `crates/luft-adapters/src/acp_adapter.rs`)

```rust
fn skill_dirs_for_backend(backend_id: &str) -> &'static [&'static str] {
    // No `.loom/skills` — luft doesn't spawn loom as an ACP backend at all.
    // loom consumes this skill from the other direction, as a library
    // dependency reading `luft_planner::WORKFLOW_SKILL` (§4.1's library
    // channel in skill-architecture.md).
    match backend_id {
        "codex" | "opencode" => &[".agents/skills"],
        "claude" | "claude-code" => &[".claude/skills"],
        _ => &[],
    }
}

fn write_workflow_skill_files(backend_id: &str, working_folder: &Path) {
    for base in skill_dirs_for_backend(backend_id) {
        let skill_dir = working_folder.join(base).join("workflow");
        if let Err(e) = write_skill_to_dir(&skill_dir, &luft_planner::WORKFLOW_SKILL) {
            tracing::warn!(dir = %skill_dir.display(), error = %e, "failed to write workflow skill");
        }
    }
}

fn write_skill_to_dir(dir: &Path, skill: &luft_core::contract::skill::Skill) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("SKILL.md"), skill.content)?;
    for (rel_path, content) in skill.references {
        let path = dir.join(rel_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
    }
    Ok(())
}
```

`luft-adapters/Cargo.toml` gained a new dependency edge: `luft-planner` (for `WORKFLOW_SKILL`). No cycle — `luft-planner` doesn't depend on `luft-adapters`.

### 4.3 Call site (implemented)

```rust
write_workflow_skill_files(config.id, &state.cwd);
```

No new MCP server, no ACP protocol field — purely a filesystem side effect the spawned process's own skill-discovery (native to codex/opencode/loom) picks up on its own.

### 4.4 Tests (implemented, 6 tests in `acp_adapter.rs`'s test module)

- `skill_dirs_for_known_backends` / `skill_dirs_for_unknown_backend_is_empty`
- `write_skill_to_dir_writes_main_and_references`
- `write_workflow_skill_files_only_writes_the_matching_backend_dir` — a `codex` call must NOT produce a `.claude/skills` directory
- `write_workflow_skill_files_unknown_backend_writes_nothing`
- `write_workflow_skill_files_content_matches_workflow_skill`

No test asserts on ACP wire traffic — this step is pure filesystem, decoupled from the session handshake (unlike the MCP-server injection, which does touch the wire). An end-to-end test that spawns a real backend and verifies the sub-agent actually *reads* the written skill (via captured `AgentEvent::AcpRaw` tool-call events) was discussed and **is not written** — it would need a live `codex`/`opencode` run plus a task whose prompt genuinely calls for workflow-authoring knowledge.

---

## 5. Out of scope for this pass (tracked, not blocking)

- MCP `workflow://reference/{name}` resource (§0 row 3)
- Claude Code as a spawnable ACP backend (§0 row 5) — `skill_dirs_for_backend` already maps `"claude"`/`"claude-code"` → `.claude/skills` (§4.2) so no further change is needed there the day it becomes spawnable
- Any text-level tightening/rewriting of the split content beyond mechanical extraction (`skill-architecture.md` §3's "two separate steps" constraint)
- The skill-activation end-to-end test (§4.4) — discussed, **still not written**, not blocked on anything
- A `dyn SessionAsset` abstraction over MCP-injection and skill-injection (§0 row 7) — kept as two plain functions instead

---

## 6. Execution checklist

- [x] Split `lua_dsl_reference.md` into `skill/main.md` + 6 `skill/references/*.md` per §1.1, fixing "Maestro" → "Luft" in the process
- [x] Add `const_format` dependency to `luft-planner/Cargo.toml`
- [x] Rewrite `luft-planner/src/lib.rs`'s constants per §1.3
- [x] Add the reconciliation test (§1.4)
- [x] Delete `crates/luft-mcp/src/lua_dsl_reference.md`, edit `resources.rs` per §2.2
- [x] Delete `crates/luft-cli/src/lua_dsl_reference.md`
- [x] §4.0 resolved: write per-backend, unconditional on every spawn (no task-content gating)
- [x] Add `skill_dirs_for_backend`, `write_workflow_skill_files`/`write_skill_to_dir` to `acp_adapter.rs`, wire into the session-setup flow (§4.3)
- [x] Add the five unit tests in §4.4 (e2e test still not written — separate item)
- [x] `cargo check --workspace` + full test suite across `luft`/`luft-core`/`luft-mcp`/`luft-planner`/`luft-adapters`

---

## 7. Related

- [`docs/design/skill-architecture.md`](./skill-architecture.md) — the design this implements
- [`crates/luft-core/src/contract/skill.rs`](../../crates/luft-core/src/contract/skill.rs) — `Skill` struct (already shipped)
- [`crates/luft-planner/src/lib.rs`](../../crates/luft-planner/src/lib.rs) — `WORKFLOW_SKILL` (already shipped, content/references get rewired here)
