# AGENTS.md

Guidance for AI agents working on the Luft codebase.

## Project Overview

Luft is a Lua-based multi-agent orchestration runtime. Users define complex
multi-agent workflows as concise Lua scripts; the runtime handles scheduling,
concurrency, checkpointing, and progress tracking automatically.

Key capabilities:
- Sandboxed Lua VM (no `io`, `os`, `require`, or shell access)
- Checkpoint / resume for long-running workflows
- Backend-agnostic AI provider support (ACP, mock, custom)
- MCP server (stdio JSON-RPC) for integration with AI clients
- Daemon mode for persistent WebSocket-based execution

## Workspace Structure

| Crate | Description |
|-------|-------------|
| `luft-core` | Core contracts — `AgentBackend` trait, event types, skill model, scheduling, journaling, state management. **Contracts are frozen; breaking changes require a major version bump.** |
| `luft-runtime` | Lua orchestration VM with sandboxed script execution and SDK primitives (`agent`, `parallel`, `pipeline`, `converge`). |
| `luft-storage` | SQLite-based persistence for runs, events, and checkpoints. |
| `luft-adapters` | Backend adapters (ACP/agent-client-protocol, mock implementations). |
| `luft-planner` | Natural-language to Lua workflow planner via LLM. |
| `luft-skills` | Compiled-in workflow authoring skill for Luft agents. |
| `luft-service` | Unified API surface for CLI, MCP, and library consumers. |
| `luft-mcp` | MCP server (stdio JSON-RPC) exposing workflow execution tools. |
| `luft-daemon` | Background daemon with WebSocket server for persistent execution. |
| `luft-cli` | The main `luft` CLI binary. |
| `luft` | Main library facade providing easy-to-use API. |

## Build & Development Commands

```bash
# Build (release)
cargo build --release

# Run all tests
cargo test --workspace

# Lint (CI gate — zero warnings allowed)
cargo clippy --workspace --all-targets -- -D warnings

# Format check
cargo fmt -- --check

# Run CLI directly (dev)
cargo run -p luft-cli -- <command>
```

## Testing Conventions

- **Integration tests** live in `crates/<crate>/tests/*.rs`.
- **Unit tests** are inline `#[cfg(test)] mod tests` blocks.
- Test isolation: `tempfile` for filesystem, `serial_test` for sequential
  execution, `LUFT_HOME=<tempdir>` for daemon PID-file isolation.
- `MockBackend` (in `luft-core`, behind `testing` feature) provides
  deterministic, hermetic test environments.
- Use `LUFT_MOCK_BEHAVIOR=hang|error` to force specific mock behaviors.

## Project Conventions

### Error Handling
- `thiserror` with `#[derive(Error)]` for typed library errors.
- `anyhow::Result` for application-level error propagation.

### Logging
- Structured logging via `tracing` + `tracing-subscriber` (env-filter).
- **Never use `eprintln!` or `println!` for diagnostics.**
- Default log file: `~/.luft/logs/luft.log` (overridable via CLI `--log-file`
  or config `[log].file`).
- Daemon background mode (non-`--foreground`) redirects stdout/stderr to
  `~/.luft/logs/daemon.log`.

### Async Runtime
- Tokio with `full` features.
- Use `CancellationToken` for cooperative cancellation.
- Use `tokio::sync::broadcast` for event fan-out.

### Code Style
- Rust edition 2021, workspace version managed centrally.
- Zero clippy warnings required (`-D warnings`).
- Comments are for non-obvious logic only — do not add redundant comments.
- Prefer `is_some_and` over `map_or(false, ...)`.

### Sandbox Model
Lua workflow scripts are fully sandboxed — no file, network, or shell access.
All I/O flows through Rust host functions exposed to the VM.

### Feature Flags
- `testing` — exposes mock utilities for test consumers.
- `unstable_end_turn_token_usage` — experimental token usage tracking.

## CI

CI runs on Ubuntu, Windows, and macOS. All three platforms must pass:
format check, clippy (`-D warnings`), and full test suite. Release binaries
are built as artifacts.

## Environment Notes

- **Windows**: PowerShell only (`bash` unavailable). Use `;` instead of `&&`.
- LLM provider: OpenAI-compatible endpoint (configurable via `OPENAI_API_KEY`
  and `OPENAI_BASE_URL`).
- The daemon auto-detects available backends; when none are found it falls
  back to `mock`.
