# Contributing to Maestro

## Development Setup

```bash
git clone <repo-url>
cd maestro
cargo build --workspace
cargo test --workspace
```

### Prerequisites

- Rust 1.75+ (stable)
- A running `opencode acp` instance (for ACP backend integration tests)

## Before Submitting a PR

1. **Format**: `cargo fmt --all`
2. **Lint**: `cargo clippy --workspace --all-targets -- -D warnings`
3. **Test**: `cargo test --workspace`
4. **Docs**: `cargo doc --workspace --no-deps` (must produce zero warnings)

## Architecture Overview

```text
  ┌──────────────────────────────────────────────────────────┐
  │                      maestro (facade)                     │
  │  MaestroBuilder · Maestro · RunHandle · RunOutcome        │
  └───────┬──────────┬──────────┬─────────┬────────┬─────────┘
          │          │          │         │        │
     ┌────▼────┐ ┌───▼───┐ ┌───▼────┐ ┌──▼───┐ ┌──▼──────┐
     │  core   │ │runtime│ │storage │ │planner│ │ adapters│
     │contracts│ │ Lua VM│ │ SQLite │ │NL→Lua│ │   ACP   │
     └─────────┘ └───────┘ └────────┘ └──────┘ └─────────┘
```

| Crate | Role |
|-------|------|
| `maestro` | Facade: re-exports all sub-crates, provides `Maestro` builder API |
| `maestro-core` | Frozen contracts: `AgentBackend` trait, types, scheduler, journal |
| `maestro-runtime` | Sandboxed mlua VM with orchestration SDK primitives |
| `maestro-storage` | SQLite persistence with UI-ready query API |
| `maestro-planner` | Natural-language → Lua script generation |
| `maestro-adapters` | OpenCode ACP backend implementation |
| `maestro-service` | Presentation-free run lifecycle and query functions |

## Adding a New Agent Backend

Implement the `AgentBackend` trait from `maestro-core`:

```rust
use maestro_core::contract::backend::*;
use async_trait::async_trait;

struct MyBackend;

#[async_trait]
impl AgentBackend for MyBackend {
    fn id(&self) -> &'static str { "my-backend" }
    fn capabilities(&self) -> AgentCapabilities { AgentCapabilities::default() }
    async fn run(&self, task: AgentTask, ctx: RunContext) -> Result<AgentResult, BackendError> {
        todo!()
    }
    fn as_any(&self) -> &dyn std::any::Any { self }
}
```

Register it with the `BackendRegistry`:

```rust
use maestro_core::BackendRegistry;
use std::sync::Arc;

let mut registry = BackendRegistry::new();
registry.register(Arc::new(MyBackend));
```

See `maestro-adapters/src/acp_adapter.rs` for a production reference implementation.

## Adding a Lua SDK Primitive

1. **Register** the global in `maestro-runtime/src/sdk/` — call `globals.set(name, callback)`.
2. **Document** it in `maestro-planner/src/lua_dsl_reference.md` — the DSL spec sent to the planner LLM.
3. **Test** it in `maestro-runtime/src/sdk/` with unit tests.
4. **Validate** — if the primitive has structural requirements (e.g., span pairing),
   add checks to `validate_workflow()` in `maestro-runtime/src/sandbox.rs`.

## Coding Conventions

- **Error handling**: use `thiserror` for library error types, `anyhow` for
  application-level plumbing. Public error enums should be `#[non_exhaustive]`.
- **Async**: use `tokio` runtime. All async public functions must be `Send`.
- **Visibility**: only mark items `pub` if they are part of the public API.
  Prefer `pub(crate)` for internal items.
- **Documentation**: every `pub` item should have a `///` doc comment.
- **Testing**: co-locate unit tests in `#[cfg(test)] mod tests`. Integration
  tests go in `tests/` directories.

## Release Process

1. Update `CHANGELOG.md` under `[Unreleased]` → rename to `[x.y.z] - YYYY-MM-DD`.
2. Update `version` in all `Cargo.toml` files (workspace `version.workspace = true`).
3. Run `cargo test --workspace` and `cargo doc --workspace --no-deps`.
4. Tag: `git tag vx.y.z && git push --tags`.
5. CI publishes to crates.io in dependency order.
