# Claude Code Dynamic Workflows

## Executive Summary

- Dynamic Workflows move orchestration from Claude's conversational turn-by-turn loop into a script executed in a sandboxed runtime, solving context saturation, non-repeatability, and lack of quality guarantees.
- The "who holds the plan" framework distinguishes workflows (plan held in code, results in script variables) from subagents, skills, and agent teams (plan held by Claude, results in context window).
- The script itself has no filesystem, shell, or network access — all I/O is delegated to subagents spawned via `agent()` calls.
- Deterministic resumption within a session is achieved via a journal of completed agent results keyed by a hash of the task; completed agent calls skip re-execution on resume.
- Core primitives include `agent()`, `parallel()` (barrier), `pipeline()` (streaming stages), `phase()`, structured output via JSON Schema, budget-aware execution, and nested `workflow()` calls.
- Three known limitations persist: no mid-run human-in-the-loop, one-shot LLM script generation without a test harness, and coarse cost control without runtime-adaptive mechanisms.
- The convergent-adversarial quality pattern lacks published comparative benchmarks against human-labeled accuracy.

## Background & Definition

A **Dynamic Workflow** is a script that Claude writes on the fly for a given task and that a sandboxed runtime executes, orchestrating dozens to hundreds of subagents in parallel. Intermediate results live in script variables, not the model's context window; only the final consolidated answer returns to the conversation.

The framework that distinguishes the four mechanisms hinges on **"who holds the plan"** — who decides what runs next and where intermediate state lives:

| Mechanism | Who decides next step | Where intermediate results live | Scale |
|---|---|---|---|
| **Subagent** | Claude, turn by turn | Claude's context window | A few per turn |
| **Skill** | Claude, following instructions | Claude's context window | Same as subagents |
| **Agent Team** | Lead agent, turn by turn | Shared task list | A handful of long-running peers |
| **Workflow** | **The script** (code) | **Script variables** | 10s–100s per run, up to 1000 |

With subagents, skills, and agent teams, Claude is the orchestrator: it decides turn by turn what to spawn next, and every result lands back in its context window. A workflow moves the plan into code — the script holds loops, branches, fan-out, fan-in, and results, so Claude's context holds only the final answer.

This paradigm shift solves three fundamental bottlenecks:

1. **Context saturation** — In the conversational model, each subagent's result accumulates in the context window until it overflows. A workflow's intermediate state lives in script variables (effectively unbounded), so it can coordinate hundreds of agents without exhausting Claude's context.

2. **Non-repeatable orchestration** — Turn-by-turn orchestration is ephemeral; the same task run twice may take entirely different paths. A workflow script is deterministic code — the same input produces the same execution path — making it re-runnable, resumable (within session), auditable via diff, and saveable as a `/command` for reuse.

3. **No built-in quality guarantee** — Single-pass LLM calls have no cross-validation. Workflows codify quality patterns like adversarial convergence: multiple independent producers generate findings, adversary agents try to refute each one, and only survivors after iterative voting rounds are reported.

## Architecture & Execution Model

**Script Sandbox.** Claude Code executes JavaScript in a Node `vm` sandbox, further isolated via OS-level mechanisms (`bubblewrap` on Linux, `Seatbelt` on macOS). The script cannot access `fs`, `child_process`, or network APIs directly. All side effects go through agent calls.

**Deterministic Resumption via Journal.** Resumption relies on a journal that stores completed agent results keyed by cache key hash. On resume, the runtime rebuilds the index from the checkpoint file. When `agent()` is called, it first queries the journal — a hit skips execution and returns the cached result directly. This resumption is **session-scoped** (cache lost on exit). The `parallel()` primitive applies the same cache-check-per-item logic before submitting any batch to the scheduler. Claude Code's cache mechanism is prefix-based prompt caching at the API level.

## Core Primitives / Capabilities

Official Claude Code documentation describes the workflow concept conceptually. The following primitives are derived from documented behavior and community extensions.

**`agent(prompt, opts?)`** — the core subagent spawner. Returns the agent's final text as a string, or a validated object if a JSON Schema is provided. Additional opts include `label`, `phase`, and `model`.

**`parallel(thunks)`** — a barrier: spawns all thunks concurrently, awaits every one before returning. A failing thunk resolves to `null` (never rejects).

**`pipeline(items, stages)`** — multi-stage processing with real streaming (item A can be in stage 3 while item B is in stage 1).

**`phase(title)`** — progress grouping for the TUI. Subsequent `agent()` calls are grouped under this title.

**`export const meta = { name, description, phases }`** — required script preamble. Must be a pure literal (no variables, interpolation, spreads).

**Structured Output (JSON Schema)** — enforced at the tool-call layer with retries on schema mismatch.

**`workflow(nameOrRef, args?)`** — nested workflow invocation. Shares the parent's concurrency cap and abort signal.

**`args`** — parameter injection into saved workflows. Passed directly as an array/object.

**Built-in `/deep-research`** — fans out web searches across several angles, fetches and cross-checks sources, votes on each claim adversarially, returns a cited report with refuted claims filtered out (requires WebSearch tool).

**Trigger Mechanisms.** (1) **`ultracode`** — `/effort ultracode` combines `xhigh` reasoning effort with automatic workflow orchestration for every substantive task. Persists for the session. (2) **Natural language** — including the keyword `workflow` (or `ultracode`) in the prompt. (3) **`/workflows` command** — lists running/completed runs; `s` saves a run as a reusable command.

**Execution Constraints.** Max 16 concurrent agents (fewer on low-core machines), 1000 agents total per run. No filesystem/shell access from the script itself. Scripts are plain JavaScript (no TypeScript syntax). Resume via `resumeFromRunId` (session-scoped).

## Limitations & Open Questions

**No mid-run user input.** Official Claude Code explicitly documents this — subagents always run in `acceptEdits` mode, and permission prompts for shell/network can fire but there is no workflow-level pause for human input.

**Script quality depends on one-shot LLM generation.** The orchestration script is written by the LLM with no test harness. Claude Code lets users view/edit the script before launch.

**Coarse cost control.** Claude Code offers agent caps (16 concurrent, 1000 total) and no programmatic budget API.

**Evaluation gaps.** The convergent-adversarial pattern (producer → adversary → vote → survive) lacks published comparative benchmarks against human-labeled accuracy across task types. Defaults (vote threshold 0.7, max 3 rounds) have no published accuracy data.

## Confidence & Caveats

The following claims in the draft could not be verified or are known to be inaccurate:

1. **"V8 isolate" sandbox** — The draft claims Claude Code uses a "V8 isolate." Official docs describe Node `vm` sandboxing + OS-level isolation (bubblewrap/Seatbelt). The term "V8 isolate" is not used in any official documentation and is misleading.

2. **Cache key `blake3(backend_id + model + prompt + phase)`** — This formula is from a third-party implementation, not official Claude Code. Claude Code uses standard Anthropic prompt caching (prefix-based), not a custom blake3-based task hash.

3. **Primitives `agent()`, `parallel()`, `pipeline()`, `phase()` as a documented Claude Code API** — These exact function names are documented in community forks. Official Claude Code documentation does not publish a specific JavaScript function-level API with these signatures; it describes the workflow concept conceptually.

4. **`budget` with `{ total, spent(), remaining() }` in Claude Code** — This signature exists in community forks only. Official Claude Code does not document a programmatic budget API.

5. **"Bun Zig→Rust port (~750K lines, 99.8% tests passing, 11 days)"** — This case study is cited from Anthropic's official blog and the Bun PR #30412. It appears credible but could not be independently verified against real-time data in this review. The statistic is widely reported across multiple sources.

6. **`Date.now()`, `Math.random()`, and argless `new Date()` throw** — This behavior is specific to community forks' determinism enforcement. Official Claude Code docs do not specify which globals are restricted.
