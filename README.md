# Luft

A Lua-based multi-agent orchestration runtime. Define complex multi-agent workflows as concise Lua scripts — the runtime handles scheduling, concurrency, checkpointing, and progress tracking automatically.

## Install

```bash
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/hi-youichi/luft/main/install.sh | sh

# Windows (PowerShell)
irm https://raw.githubusercontent.com/hi-youichi/luft/main/install.ps1 | iex

# Specific version
curl -fsSL https://raw.githubusercontent.com/hi-youichi/luft/main/install.sh | sh -s -- --version v0.4.2

# From source
cargo install luft-cli
```

Verify:

```bash
luft --version
```

## Quick Start

Run an example workflow with the mock backend (no LLM required):

```bash
luft run --workflow examples/hello.lua --backend mock
```

Natural-language prompt — Luft generates a workflow plan via LLM, then executes it:

```bash
luft run "audit the codebase for security issues" -o report.md
```

Run a saved workflow with arguments:

```bash
luft run --workflow workflows/review_code.lua --args '{"target":"src/"}' --max-concurrency 4
```

## How It Works

You write a **Lua orchestration script** that spawns AI subagents to do the real work (reading files, writing code, web search, etc.). The script itself runs in a sandbox with no filesystem or shell access — it only holds the control flow, branching, and intermediate results.

```
┌──────────────────────────────────────────┐
│           User (CLI / Library / MCP)        │
├──────────────────────────────────────────┤
│         Lua Orchestration Runtime           │
│   agent · parallel · pipeline · phase      │
├──────────────────────────────────────────┤
│            Service Layer                    │
│   scheduling · checkpointing · events       │
├──────────────────────────────────────────┤
│            Backend Adapters                 │
│   OpenCode · Claude · Codex · Custom        │
└──────────────────────────────────────────┘
```

**Key properties:**
- **Sandboxed scripts** — no `io`, `os`, `require`, or shell access from Lua
- **Checkpoint & resume** — every run can be resumed from its last checkpoint
- **Progress tracking** — phases, agent counts, token usage, elapsed time
- **Backend-agnostic** — switch between AI providers without changing workflows
- **MCP server** — control workflows programmatically via JSON-RPC

## Orchestration Primitives

| Primitive | Description |
|-----------|-------------|
| `agent(opts)` | Run a single subagent to completion — the fundamental work unit |
| `parallel(items, fn)` | Fan-out: run agents for all items, wait for every result |
| `pipeline{items=, stages=, max_inflight=}` | Streaming multi-stage processing with bounded concurrency |
| `phase(name)` | Declare a progress phase for CLI display |
| `log(msg)` | Emit a status line to CLI and event log |
| `budget(time_ms?, max_rounds?)` | Set resource-limit hints |
| `workflow(path, args?)` | Call another saved workflow as a sub-step |
| `report(value)` | **Required** — set the final output (call exactly once) |
| `json.encode(v)` / `json.decode(s)` | JSON helpers |

## Example: Parallel Code Review

```lua
--------------------------------------------
-- Goal:  Review source files in parallel
-- Arch:  files ==> parallel-review ==> report
-- Flow:  files[] -> results[] -> report
--------------------------------------------
meta = {
    reasoning = "Fan out file review across agents, collect findings",
    phases = {
        { label = "review", dynamic = true },
        { label = "report" },
    },
}

local FILES = { "src/main.rs", "src/lib.rs", "src/cli.rs" }

function main()
    phase("review", #FILES)

    local results = parallel(FILES, function(file)
        return agent({
            prompt = "Review " .. file .. " for security issues. "
                .. "Report any vulnerabilities found.",
        })
    end)

    phase("report")

    local findings = {}
    for i, r in ipairs(results) do
        if r.ok then
            table.insert(findings, { file = FILES[i], output = r.output })
        end
    end

    report({
        summary = "Reviewed " .. #FILES .. " files, "
            .. #findings .. " returned results",
        results = findings,
    })
end
```

More examples in [`examples/`](examples/):
- [`hello.lua`](examples/hello.lua) — simplest single-agent call
- [`parallel-demo.lua`](examples/parallel-demo.lua) — parallel fan-out
- [`pipeline-demo.lua`](examples/pipeline-demo.lua) — streaming pipeline
- [`schema-demo.lua`](examples/schema-demo.lua) — structured output with schemas

## CLI Reference

| Command | Description |
|---------|-------------|
| `luft run --workflow <file>` | Execute a Lua workflow script |
| `luft run "<prompt>"` | Generate a workflow from natural language, then execute |
| `luft run --resume` | Resume from the last checkpoint |
| `luft run -o <file>` | Write the final report to a file |
| `luft run --args '<json>'` | Pass arguments to the workflow |
| `luft run --max-concurrency N` | Max parallel agents (default: 1) |
| `luft generate "<prompt>"` | Generate a workflow script without executing |
| `luft list` | List past runs |
| `luft status <run-dir>` | Show run status and results |
| `luft logs <run-dir>` | View event log for a run |
| `luft phases <run-dir>` | Show planned phases |
| `luft backend list` | List available AI backends |
| `luft skill-dump <dir>` | Dump the built-in workflow skill to a directory |
| `luft install` | Install Luft bridges for detected agents |

## Embed as a Library

```rust
use luft::Luft;

#[tokio::main]
async fn main() -> Result<(), luft::LuftError> {
    let luft = Luft::builder()
        .backend(MyBackend::new())
        .build()?;

    let outcome = luft.run_script(r#"
        function main()
            local result = agent({ prompt = "analyze code security" })
            report({ findings = result.output })
        end
    "#).await?;

    println!("{:?}", outcome.result);
    Ok(())
}
```

## MCP Integration

Luft includes a built-in MCP (Model Context Protocol) server. Any MCP-compatible agent can submit workflows, poll status, and read results:

```bash
luft mcp serve
```

Available MCP tools: `workflow_execute`, `workflow_status`, `workflow_events`, `workflow_cancel`, `workflow_list_files`, `workflow_list_runs`.

## Workspace Layout

| Crate | Role |
|-------|------|
| `luft-core` | Core contracts: `AgentBackend` trait, event types, skill model |
| `luft-runtime` | Lua sandbox, scheduling, pipeline, checkpoint engine |
| `luft-storage` | SQLite-based persistence (runs, events, checkpoints) |
| `luft-adapters` | Backend adapters (OpenCode, Claude, Codex, mock) |
| `luft-planner` | NL-to-Lua planning via LLM |
| `luft-skills` | Compiled-in workflow authoring skill |
| `luft-service` | Unified API surface for CLI, MCP, and library consumers |
| `luft-mcp` | MCP server (stdio JSON-RPC) |
| `luft-daemon` | Background daemon for persistent workflow execution |
| `luft-cli` | The `luft` binary |

## Resources

- [Architecture overview](docs/architecture.md)
- [SDK reference](docs/sdk-reference.md)
- [Library guide](docs/library-guide.md)
- [Tool reference](docs/tool-reference.md)
- [Design documents](docs/design/)
- [Example workflows](examples/)

## License

MIT
