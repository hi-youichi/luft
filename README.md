# Luft

Lua-based multi-agent orchestration runtime. Use Lua scripts with `agent()`/`parallel()`/`pipeline()`/`converge()` primitives to deterministically orchestrate multiple LLM agents.

## CLI

```bash
cargo run --bin luft -- run --workflow examples/hello.lua --backend mock
cargo run --bin luft -- run "audit repo for security issues" -o report.md
```

## Library

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

## Commands

| Command | Description |
|---------|-------------|
| `run --workflow <file>` | Execute a Lua workflow |
| `run "<NL>"` | Natural language → Lua → execute |
| `run --resume` | Resume from checkpoint |
| `run -o <file>` | Write report to file |
| `run --args <JSON>` | Pass arguments to workflow |
| `list` / `status` / `logs` | View run history |

## Examples

`examples/hello.lua` · `parallel-demo.lua` · `pipeline-demo.lua` · `converge-demo.lua` · `deep-research.lua`