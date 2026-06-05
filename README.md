# Maestro

基于 Lua 的多智能体编排运行时。用 Lua 脚本调用 `agent()`/`parallel()`/`pipeline()`/`converge()` 等 SDK 原语，确定性编排多个 LLM agent 协作完成复杂任务。

对标 [Claude Code Dynamic Workflows](https://code.claude.com/docs/en/workflows)，核心能力相当，并在 pipeline（多阶段流式）和 converge（对抗性验证）上有所超越。

## Quick Start

```bash
# 用 mock 后端运行示例（确定性、零成本，验证编排流程）
cargo run --bin maestro -- run --workflow examples/hello.lua --backend mock

# 自然语言直接运行（自动探测 opencode 后端）
cargo run --bin maestro -- run "审计仓库安全问题" -o report.md

# 列出历史运行
cargo run --bin maestro -- run list   # 或: cargo run --bin maestro -- list

# 查看运行状态
cargo run --bin maestro -- status <run_id>
```

## Deep Research（深度研究）

`examples/deep-research.lua` 是一个端到端的多智能体深度研究工作流：
**分解 → 并行调研 → 综合 → 校验润色**。一个首席研究员把主题拆成若干子问题，
每个子问题派一个子研究员并行调研（可读取本仓库 `./docs` 下资料夯实结论），
再由分析师综合、技术编辑做事实核查，最终产出一篇 Markdown 报告。

```bash
# 默认主题 = "Claude Code Dynamic Workflows"，用真实 LLM 后端 opencode 运行，
# 把最终报告写到一个干净的 .md 文件：
cargo run --bin maestro -- run \
    --workflow examples/deep-research.lua \
    --backend opencode \
    -o claude-code-dynamic-workflows.md

# 自定义主题 / 广度（子问题数量）：
cargo run --bin maestro -- run \
    --workflow examples/deep-research.lua \
    --backend opencode \
    -o rust-async.md \
    --args '{"topic":"Rust async runtimes","breadth":5}'
```

执行期间进度（phase / agent / log）实时打到 **stderr**；`-o <file>` 写出报告：
若 `report()` 的返回值含 `markdown` 字段，则直接写出该 Markdown，否则写 pretty JSON。
一次真实运行产出的示例报告见
[docs/research/claude-code-dynamic-workflows-deep-research.md](docs/research/claude-code-dynamic-workflows-deep-research.md)。

> 需要本机已安装并配置好 `opencode`（`opencode auth login`）。`--backend mock`
> 只产出占位输出，仅用于验证编排流程，不做真实研究。

## 核心概念

| 概念 | 说明 |
|------|------|
| **Lua 沙箱编排** | mlua 5.4 VM 运行用户脚本，屏蔽 io/os/fs/network 危险全局 |
| **可插拔后端** | `AgentBackend` trait 支持 Claude Code / MCP / WebSocket 等多种 LLM 后端 |
| **收敛验证** | producer → adversary → voting → 收敛，确保输出质量 |
| **进度持久化** | blake3 cache key + JournalStore + JSONL 事件日志，中断可恢复 |

## 命令一览

| 命令 | 状态 | 说明 |
|------|------|------|
| `maestro run --workflow <file>` | ✅ | 执行 Lua 工作流 |
| `maestro run --headless` | ✅ | JSONL 事件流输出 |
| `maestro run --resume` | ✅ | 从断点恢复 |
| `maestro run --confirm` | ✅ | 执行前展示生成脚本并等待确认（默认自动执行） |
| `maestro run -o <file>` | ✅ | 把最终报告写到文件（含 `markdown` 字段则写干净 Markdown，否则 pretty JSON） |
| `maestro run --args <JSON>` | ✅ | 以 JSON 对象向工作流传参（`args.*`），如 `--args '{"topic":"..."}'` |
| `maestro run "<NL>"` | ✅ | 自然语言 → Lua（agent 驱动的 planner），用法示例：`maestro run "审计仓库安全问题" -o report.md` |
| `maestro list` | ✅ | 列出历史运行 |
| `maestro status <id>` | ✅ | 运行状态 + token 用量 |
| `maestro logs <id>` | ✅ | 事件流日志 |
| `maestro watch <id>` | ❌ | TUI 实时监控（未实现） |

## 文档

### 活跃文档（日常维护）

| 文档 | 内容 |
|------|------|
| [**Dynamic Workflow 指南**](docs/dynamic-workflow-guide.md) | **由浅入深：范式直觉、运转机制、设计权衡、Claude Code vs Maestro 对比、未解决的挑战** |
| [技术指南](docs/technical-guide.md) | 各模块的技术设计动机、关键算法、数据结构、实现细节 |
| [架构设计](docs/architecture.md) | 架构总览 + 模块索引：核心抽象、数据流、依赖关系 |
| [Lua SDK 参考](docs/sdk-reference.md) | 10 个原语的参数、返回值、代码示例 |
| [路线图](docs/roadmap.md) | v0.1 → v0.2 实施计划、技术选型 |

### 模块架构（按模块拆分，对齐当前代码）

| 文档 | 内容 |
|------|------|
| [core](docs/architecture/core.md) | 冻结合约 + 调度器 + 状态持久化 + journal/resume |
| [runtime](docs/architecture/runtime.md) | mlua 沙箱、block_on 执行模型、pipeline、converge |
| [adapters](docs/architecture/adapters.md) | AcpAdapter、!Send 线程桥接、ACP 会话、权限决策 |
| [planner](docs/architecture/planner.md) | NL → Lua、生成-校验回环、DSL 规范 |
| [mcp](docs/architecture/mcp.md) | MCP 数据面 server、上报工具、已建未联现状 |
| [cli](docs/architecture/cli.md) | run 生命周期编排、TUI/headless、只读命令 |

### 设计文档（按模块拆分，来自 v0.1 代码设计）

| 文档 | 内容 |
|------|------|
| [冻结合约](docs/design/contracts.md) | 核心类型、AgentBackend trait、事件枚举 |
| [调度器](docs/design/scheduler.md) | Scheduler 并发/配额/重试/取消 |
| [状态落盘](docs/design/state.md) | StateStore、blake3 缓存键、恢复时序 |
| [运行时](docs/design/runtime.md) | mlua 沙箱、协程驱动、SDK 桥接 |
| [MCP 数据平面](docs/design/mcp-server.md) | MCP server、ResultCollector、工具定义 |
| [后端适配器](docs/design/backends.md) | AcpAdapter、ACP 协议流程、权限控制 |
| [规划器](docs/design/planner.md) | NL → Lua、LLM 重试回路 |
| [CLI](docs/design/cli.md) | clap 参数、TUI、headless、工作流管理 |
| [集成与测试](docs/design/integration-testing.md) | 跨模块接缝、测试分层、CI 策略 |

### 研究

| 文档 | 内容 |
|------|------|
| [Claude 对标研究](docs/research/claude-dynamic-workflow-analysis.md) | Claude Code Dynamic Workflows 特性分析 |

### 归档（冻结只读）

| 文档 | 内容 |
|------|------|
| [v0.1 产品技术设计](docs/archive/v0.1-product-tech-design.md) | 早期设计文档 |
| [v0.1 开发计划](docs/archive/v0.1-dev-plan.md) | 开发阶段产物 |
| [v0.1 审计快照](docs/archive/v0.1-status-snapshot.md) | 完成度快照 |
| [Claude DW 参考](docs/archive/claude-dw-reference.md) | Claude Code 研究笔记 |

## Natural Language → Lua（自然语言编排）

用一句自然语言描述任务，planner 自动生成 Lua 工作流并执行。需配合真实 LLM 后端（`opencode`）：

```bash
# 自然语言 → 自动生成 Lua 脚本 → 执行
maestro run "审计这个仓库的安全问题" -o security-audit.md

# 生成架构概览报告
maestro run "列举所有模块及其职责，生成架构概览" -o architecture.md

# 查看生成脚本并确认后再执行
maestro run "分析代码质量" --confirm
```

Planner 会自动规划阶段（如 discovery → analysis → synthesis），生成完整的 Lua 编排脚本，经语法校验后交由运行时执行。
生成质量取决于后端模型能力，内置最多 3 次自纠错重试。
NL 模式自动探测可用后端（优先 opencode）；若未安装则提示安装。

## 示例

| 文件 | 展示 |
|------|------|
| `examples/hello.lua` | 最简 agent 调用 |
| `examples/parallel-demo.lua` | 并行处理多个任务 |
| `examples/pipeline-demo.lua` | 多阶段流式管道 |
| `examples/converge-demo.lua` | 对抗性收敛验证 |
| `examples/deep-research.lua` | **深度研究：分解 → 并行调研 → 综合 → 校验，产出 Markdown 报告** |

## 技术栈

Rust · mlua 5.4 · tokio · clap · jsonschema · blake3
