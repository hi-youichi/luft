# Using Luft as a Library

Add the `luft` crate and run an orchestration script directly from Rust, without going through the CLI.

```toml
luft = { path = "../luft", features = ["testing"] }
tokio = { version = "1", features = ["full"] }
```

```rust
use luft::core::mock_backend::{MockBackend, MockBehavior};
use luft::Luft;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let m = Luft::builder()
        .backend(MockBackend::new("mock", vec![MockBehavior::Success {
            output: serde_json::json!({"message": "hello"}),
            tokens: Default::default(),
            delay: Duration::ZERO,
        }]))
        .build()?;

    let outcome = m.run_script(r#"
        function main()
            local r = agent({ prompt = "say hello" })
            report({ output = r.output })
        end
    "#).await?;

    println!("{:#?}", outcome.result?);
    Ok(())
}
```

`Luft::builder()` takes an `AgentBackend` implementation (`MockBackend` above; swap in a real backend like `luft_adapters::AcpAdapter` for production use) and returns a `Luft` facade. `run_script` executes an inline Lua script synchronously to completion and returns a `RunOutcome` whose `result` is the JSON passed to `report(...)` in the script.

For fire-and-forget execution (start a script, poll or subscribe to progress separately), use `start_script`/`start_resume`, which return a `RunHandle` instead of blocking — see [`crates/luft/src/builder.rs`](../crates/luft/src/builder.rs).

## Related

- [`docs/design/library-split.md`](./design/library-split.md) — crate boundaries (`core`/`runtime`/`storage`/`planner`/`adapters`)
- [`docs/mcp-server.md`](./mcp-server.md) — driving Luft over MCP instead of embedding it as a library
