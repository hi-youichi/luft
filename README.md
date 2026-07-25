# Luft

**Lua-based multi-agent orchestration runtime that makes complex AI tasks as simple as writing scripts.**

## 🎯 What problem does it solve?

**Pain points**:
- Single AI agents can't handle complex multi-step tasks
- Manually coordinating multiple agents is difficult and requires extensive code
- Lack of unified task orchestration and progress tracking mechanisms
- Hard to reuse and share complex workflows

**Solution**:
- **Scriptable orchestration**: Define complex multi-agent workflows with concise Lua primitives
- **Automatic scheduling**: System handles concurrency, retries, and error recovery automatically
- **Progress visualization**: Real-time tracking of execution status for each phase and agent
- **Easy sharing**: Workflows are script files that can be version controlled and collaborated on

## 🚀 Quick Start

### CLI Usage

```bash
# Execute example workflow
cargo run --bin luft -- run --workflow examples/hello.lua --backend mock

# Natural language task (auto-converted to workflow)
cargo run --bin luft -- run "analyze project security" -o report.md

# View run status
cargo run --bin luft -- status
```

### Library Usage

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

    println!("Result: {:?}", outcome.result);
    Ok(())
}
```

## 📋 Core Orchestration Primitives

| Primitive | Function | Example |
|-----------|----------|---------|
| `agent()` | Run single agent task | `agent({ prompt = "review code" })` |
| `parallel()` | Process multiple tasks in parallel | `parallel(files, function(f) return analyze(f) end)` |
| `pipeline()` | Streaming pipeline processing | `pipeline(data, {"stage1", "stage2", "stage3"})` |
| `converge()` | Multi-round consensus verification | `converge(results, { max_rounds = 3 })` |
| `workflow()` | Call nested sub-workflow | `workflow("subtask.lua", { param = "value" })` |
| `phase()` | Structured progress tracking | `phase("code review", #files)` |
| `report()` | Send final output | `report({ total_issues = 5 })` |

## 🏗️ Architecture Overview

Luft adopts a layered architecture that decouples orchestration logic from specific AI backends:

```
┌─────────────────────────────────────┐
│     User Layer (CLI / Library)      │
├─────────────────────────────────────┤
│     Orchestration Layer (Lua Runtime)│
│   - agent/parallel/pipeline/converge│
├─────────────────────────────────────┤
│     Service Layer                   │
│   - scheduling/persistence/events/  │
│     query                           │
├─────────────────────────────────────┤
│     Backend Layer (Adapters)        │
│   - OpenCode / Claude / Custom      │
└─────────────────────────────────────┘
```

**Core features**:
- **Sandbox security**: Lua sandbox ensures workflows cannot escape execution environment
- **Checkpoint recovery**: Any workflow can resume execution from breakpoints
- **Real-time monitoring**: View execution progress through event streams
- **Unified interface**: Support multiple AI backends without workflow modifications

## 💡 Usage Examples

### Parallel Code Review
```lua
local files = { "src/main.rs", "src/lib.rs", "src/cli.rs" }

function main()
    phase("parallel review", #files)
    
    local results = parallel(files, function(file)
        return { prompt = "review security of this file: " .. file }
    end)
    
    local total_issues = 0
    for _, result in ipairs(results) do
        total_issues = total_issues + #result.findings
    end
    
    report({ total_files = #files, total_issues = total_issues })
end
```

### Adversarial Verification
```lua
local claims = {
    "API endpoints need RBAC authentication",
    "Password storage uses bcrypt hashing", 
    "Input validation covers SQL injection"
}

function main()
    phase("security verification", #claims * 2)
    
    local result = converge(claims, {
        adversarial = true,
        vote_threshold = 0.7,
        max_rounds = 3
    })
    
    report(result)
end
```

### Streaming Data Analysis
```lua
local stages = {
    function(data) return extract_features(data) end,
    function(features) return analyze_patterns(features) end,
    function(patterns) return generate_report(patterns) end
}

function main()
    phase("data analysis", #stages)
    
    local result = pipeline(raw_data, stages)
    report(result)
end
```

## 🔧 Common Commands

| Command | Description | Example |
|---------|-------------|---------|
| `run --workflow <file>` | Execute Lua workflow | `luft run --workflow audit.lua` |
| `run "<task description>"` | Natural language to workflow | `luft run "audit security vulnerabilities"` |
| `run --resume <dir>` | Resume from checkpoint | `luft run --resume run-20250819-xxx` |
| `run -o <file>` | Output report to file | `luft run --workflow audit.lua -o report.md` |
| `run --args <JSON>` | Pass arguments to workflow | `luft run --workflow audit.lua --args '{"target":"src/"}'` |
| `list` | List all run records | `luft list` |
| `status <dir>` | View run status | `luft status run-20250819-xxx` |
| `logs <dir>` | View run logs | `luft logs run-20250819-xxx` |

## 🚧 Development Roadmap

### Near-term (v0.2)
- [ ] Dynamic workflow topology (generate subtasks at runtime)
- [ ] Richer backend support
- [ ] Performance optimization and cost control

### Mid-term (v1.0)  
- [ ] Intent-driven automatic workflow generation
- [ ] Intelligent task decomposition and strategy selection
- [ ] Continuous monitoring workflows

### Long-term Vision
- [ ] Evolve from orchestration tool to intelligent operating system
- [ ] Autonomous task planning and resource allocation
- [ ] Cross-project knowledge reuse and learning

## 📚 More Resources

- **Detailed documentation**: [docs/](docs/)
- **Architecture design**: [docs/architecture.md](docs/architecture.md)  
- **API reference**: [docs/sdk-reference.md](docs/sdk-reference.md)
- **Example collection**: [examples/](examples/)
- **Design documents**: [docs/design/](docs/design/)
- **Library usage guide**: [docs/library-guide.md](docs/library-guide.md)

## 🤝 Contributing

Contributions welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details.

## 📄 License

MIT License - see [LICENSE](LICENSE) for details

---

**Making complex AI task orchestration as simple as scripting.**