# Maestro

基于 Lua 的多智能体编排运行时。用 Lua 脚本调用 `agent()`/`parallel()`/`pipeline()`/`converge()` 等 SDK 原语，确定性编排多个 LLM agent 协作完成复杂任务。

## Installation

```bash
# 从源码构建（需要 Rust 1.75+）
cargo build --release
# 二进制位于 target/release/maestro
```

真实 LLM 后端需要本机已安装并配置好 `opencode`（`opencode auth login`）。`--backend mock` 仅用于零成本验证编排流程。

## Quick Start

```bash
# 用 mock 后端运行示例（确定性、零成本，验证编排流程）
cargo run --bin maestro -- run --workflow examples/hello.lua --backend mock

# 自然语言直接运行（自动探测 opencode 后端）
cargo run --bin maestro -- run "审计仓库安全问题" -o report.md

# 列出历史运行
cargo run --bin maestro -- list

# 查看运行状态
cargo run --bin maestro -- status <run_dir>
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
docs/research/claude-code-dynamic-workflows-deep-research.md。

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

| 命令 | 说明 |
|------|------|
| `maestro run --workflow <file>` | 执行 Lua 工作流 |
| `maestro run --resume` | 从断点恢复 |
| `maestro run --confirm` | 执行前展示生成脚本并等待确认（默认自动执行） |
| `maestro run -o <file>` | 把最终报告写到文件（含 `markdown` 字段则写干净 Markdown，否则 pretty JSON） |
| `maestro run --args <JSON>` | 以 JSON 对象向工作流传参（`args.*`），如 `--args '{"topic":"..."}'` |
| `maestro run --log <file>` | 将事件日志额外写入指定文件 |
| `maestro run --log-format <fmt>` | 事件日志格式（pretty / jsonl） |
| `maestro run --no-acp-raw` | 禁用原始 ACP session/update 透传（acp_raw 事件） |
| `maestro run "<NL>"` | 自然语言 → Lua（agent 驱动的 planner），用法示例：`maestro run "审计仓库安全问题" -o report.md` |
| `maestro generate "<NL>"` | 从自然语言生成 Lua 脚本（不执行），`-o` 写入文件 |
| `maestro workflows` | 列出可用工作流 |
| `maestro save <name> <output>` | 将工作流保存到文件 |
| `maestro list [--limit N]` | 列出历史运行 |
| `maestro status <run_dir>` | 运行状态 + token 用量 |
| `maestro logs <run_dir> [--limit N]` | 事件流日志 |

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
NL 模式自动探测可用后端（扫描 `opencode`、`loom-acp`）；单个可用直接使用，多个可用时交互式选择并持久化，无可用后端则回退 `mock`。

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
