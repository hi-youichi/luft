# Luft Example Workflows

This guide walks through every built-in example, from simplest to most complex.
Each step includes the command to run it and the expected output.

## Prerequisites

```bash
luft --version          # Luft installed

# For real LLM runs (not mock):
opencode --version      # OpenCode >= 1.17.0
```

## CLI Quick Reference

| Flag | Description |
|------|-------------|
| `-w, --workflow <FILE>` | Path to a Lua workflow file |
| `-b, --backend <ID>` | Backend: `mock` or `opencode` |
| `-c, --confirm` | Show script and prompt before execution |
| `-o, --output <FILE>` | Write final report to a file |
| `--log <FILE>` | Write event log to a file |
| `--log-format <FORMAT>` | Log format: `pretty` (default) or `jsonl` |
| `--args <JSON>` | Pass arguments to the workflow's `args` global |

---

## Level 1: Mock Backend (seconds)

Mock backend returns canned responses — no LLM calls required.
Useful for verifying wiring, event flow, and script correctness.

### hello.lua — Simplest Agent Call

```bash
luft run -w examples/hello.lua -b mock \
    --log .luft/example_logs/hello.jsonl --log-format jsonl
```

**Expected:** exit code 0, report contains `status == "ok"`.

### parallel-demo.lua — Parallel Fan-Out

```bash
luft run -w examples/parallel-demo.lua -b mock \
    --log .luft/example_logs/parallel.jsonl --log-format jsonl
```

**Expected:** `total_files == 3`, each `results[i].status == "ok"`.

### pipeline-demo.lua — Streaming Pipeline

```bash
luft run -w examples/pipeline-demo.lua -b mock \
    --log .luft/example_logs/pipeline.jsonl --log-format jsonl
```

**Expected:** `ok == 3`, `failed == 0`, `total_stages == 2`.

### schema-demo.lua — Structured Output

```bash
luft run -w examples/schema-demo.lua -b mock \
    --log .luft/example_logs/schema.jsonl --log-format jsonl
```

**Expected:** script completes with fallback data (mock skips real LLM extraction).

### phased_hello.lua — Multi-Phase Workflow

```bash
luft run -w examples/phased_hello.lua -b mock \
    --log .luft/example_logs/phased.jsonl --log-format jsonl
```

**Expected:** two phases execute in sequence, each reporting completion.

---

## Level 2: Real LLM Backend (minutes)

Replace `-b mock` with `-b opencode` for real agent calls.
These examples exercise the full data flow including LLM reasoning.

### schema-demo.lua — Structured Output with Schema

```bash
luft run -w examples/schema-demo.lua -b opencode \
    --log .luft/example_logs/schema-opencode.jsonl --log-format jsonl
```

**Expected:** `extracted.name` is populated from real LLM output, `summary` is non-empty.

**What it demonstrates:**
- Define a JSON schema to constrain agent output
- Access structured fields via `result.output.field_name`
- `parallel()` with per-item schemas
- Pipeline: extract → parallel validate → summarize
- `pcall`-wrapped `safe_agent` for graceful degradation

### deep-research.lua — Multi-Agent Deep Research

```bash
luft run -w examples/deep-research.lua -b opencode \
    -o deep-research.md \
    --log .luft/example_logs/deep-research.jsonl --log-format jsonl
```

**Four phases:** plan → research (parallel) → synthesize → verify

**Expected:**
- All sub-research agents succeed
- `deep-research.md` contains the title and structured sections
- Report includes a confidence/caveats section

### architecture-report.lua — Codebase Architecture Analysis

```bash
luft run -w examples/architecture-report.lua -b opencode \
    -o architecture.md \
    --log .luft/example_logs/architecture.jsonl --log-format jsonl
```

**Three phases:** discovery → analysis (parallel) → synthesis

**Expected:**
- All module analyses succeed
- `architecture.md` contains structured sections referencing concrete types
- At least core, runtime, and adapters modules are covered

---

## Event Log Reference

The `--log` flag writes JSONL — one event per line, all sharing a `run_id`.

### Lifecycle Events

| Event | `type` | Key Fields |
|-------|--------|------------|
| RunStarted | `run_started` | `task`, `ts` |
| RunDone | `run_done` | `status` (`Completed`/`Failed`/`Cancelled`), `report`, `total_tokens` |
| PhaseStarted | `phase_started` | `phase_id`, `label`, `planned` |
| PhaseDone | `phase_done` | `phase_id`, `ok`, `failed` |

### Agent Events

| Event | `type` | Key Fields |
|-------|--------|------------|
| AgentStarted | `agent_started` | `agent_id`, `prompt_preview`, `model` |
| AgentDone | `agent_done` | `status` (`Ok`/`Error`/`Timeout`), `tokens`, `elapsed_ms` |
| AgentProgress | `agent_progress` | `delta` (`Message` / `ToolCall` / `FileEdit` / `Tokens`) |

### Primitive Events

| Event | `type` | Key Fields |
|-------|--------|------------|
| BudgetSet | `budget_set` | `time_limit_ms`, `max_rounds` |
| ReportEmitted | `report_emitted` | `report` |
| ParallelStarted | `parallel_started` | `span_id`, `count` |
| ParallelDone | `parallel_done` | `span_id`, `ok`, `failed` |
| PipelineStarted | `pipeline_started` | `total_stages`, `items` |
| PipelineStageStarted | `pipeline_stage_started` | `stage_index`, `label` |
| PipelineItemDone | `pipeline_item_done` | `stage_index`, `item_index`, `status` |
| PipelineDone | `pipeline_done` | `stages_completed`, `total_ok`, `total_failed` |
| WorkflowStarted | `workflow_started` | `span_id`, `path`, `args` |
| WorkflowDone | `workflow_done` | `span_id`, `report`, `error` |

### Other Events

| Event | `type` | Key Fields |
|-------|--------|------------|
| Log | `log` | `level` (`trace`/`debug`/`info`/`warn`/`error`), `msg` |
| AcpRaw | `acp_raw` | `kind` (e.g. `agent_message_chunk`), `raw` |

### span_id Pairing

`parallel`, `pipeline`, and `workflow` events carry a `span_id` to pair Started/Done:
- The same call's Started and Done share the same `span_id`
- Different calls have different `span_id` values
- Check this pairing when debugging incomplete runs

---

## Verification Checklist

| Check | Method |
|-------|--------|
| Exit code is 0 | `echo $?` |
| `report()` was called | stdout contains `=== Report ===` or JSONL has `report_emitted` |
| Event flow is complete | JSONL ends with `run_done` where `status == "Completed"` |
| span pairing is correct | Every `*_started` has a matching `*_done` with the same `span_id` |
| No panics | stderr does not contain `panicked` |
| Token usage is non-zero | For `opencode` runs, `total_tokens > 0` |
| Report file is non-empty | `-o` output file has content |
