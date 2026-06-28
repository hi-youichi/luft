# Maestro 架构设计

> Maestro 是基于 Lua 的多智能体编排运行时。本文是**架构总览 + 模块索引**；每个顶层模块的详细架构见 [`docs/architecture/`](./architecture/) 下的专篇。

## 模块索引

| 模块 | 一句话职责 | 详细架构 |
|------|-----------|---------|
| **core** | 冻结合约 + 调度器 + 状态持久化（无上游依赖的地基层） | core.md |
| **runtime** | mlua 编排运行时：沙箱 + 10 个 SDK 原语 + pipeline + converge | [architecture/runtime.md](./architecture/runtime.md) |
| **adapters** | OpenCode ACP 真实后端（`AgentBackend` 实现） | [architecture/adapters.md](./architecture/adapters.md) |
| **planner** | 自然语言 → Lua 脚本（agent 写脚本 + 校验重试） | [architecture/planner.md](./architecture/planner.md) |
| **mcp** | MCP 数据面服务器（5 个上报工具，stdio JSON-RPC） | [architecture/mcp.md](./architecture/mcp.md) |
| **cli** | 命令行入口 + run 生命周期编排 + headless 输出 | [architecture/cli.md](./architecture/cli.md) |

## 模块布局

```
maestro
├── core/              ← 冻结合约 + 调度器 + 状态持久化（地基，无上游依赖）
│   ├── contract/      ← AgentBackend trait · AgentTask · Finding · AgentEvent · Schema · CacheKey
│   ├── scheduler/     ← 并发调度：信号量 + 配额 + 重试 + 取消 + 事件广播
│   ├── journal.rs     ← JournalStore：cache key 索引 + O(1) 查询 + ResumeContext
│   ├── state.rs       ← RunStore / RunCheckpoint：JSONL 事件日志 + checkpoint 落盘
│   └── mock_backend.rs← MockBackend：确定性测试后端
├── runtime/           ← mlua 编排运行时
│   ├── sandbox.rs     ← Lua VM 沙箱 + SDK 原语桥接 + Lua↔JSON 转换
│   ├── pipeline.rs    ← 多阶段处理引擎（当前：逐阶段栅栏 + 内联 handler）
│   ├── converge.rs    ← 对抗性收敛验证
│   └── error.rs       ← ExecLimits（已定义未强制）+ ScriptError
├── adapters/          ← AcpAdapter：opencode ACP 真实后端（client 侧）
│   ├── acp_adapter.rs ← AcpConfig + 一次性会话：spawn → init → session → prompt
│   ├── update_mapper.rs← ACP SessionUpdate → ProgressDelta + message/token 累积
│   ├── permission.rs  ← 非交互 request_permission 自动决策（纯逻辑 + 单测）
│   └── result_collector.rs← stop_reason + message → AgentResult（findings 文本回退）
├── planner.rs         ← NL → Lua 规划器（agent 生成脚本 + 语法校验/重试）
├── mcp.rs             ← MCP 数据面服务器（5 个工具 + stdio JSON-RPC）
├── cli.rs             ← run 生命周期编排（journal/scheduler/runtime 装配）
└── main.rs            ← clap 命令行入口（含 NL 规划、审批、backend 工厂）
```

## 核心抽象

三个 trait/类型构成模块间的接缝——理解它们就理解了整体协作：

### AgentBackend Trait — `core` ↔ `adapters` 接缝

所有 LLM 后端的统一接口。Prompt 进，结构化 `AgentResult` 出。

```rust
trait AgentBackend: Send + Sync {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> AgentCapabilities;
    async fn run(&self, task: AgentTask, ctx: RunContext) -> Result<AgentResult, BackendError>;
}
```

实现方：`MockBackend`（测试）、[`AcpAdapter`](./architecture/adapters.md)（opencode）。详见 core.md §3.1。

### AgentEvent 广播总线 — 唯一可观测性数据源

`tokio::sync::broadcast<AgentEvent>`。同一条流被持久化（state）、headless（JSONL）同时消费。详见 core.md §3.2。

### AgentCacheKey — `core` ↔ `runtime`（resume）接缝

blake3(prompt + model + phase) 确定性去重键。runtime 的 `agent()` 在提交调度前查 journal，命中则跳过执行。详见 core.md §3.3。

## 数据流：一次 Workflow 执行的完整路径

```
1. main.rs 解析参数 → (NL 经 planner 生成脚本 + 审批) → 构建 cli::RunArgs
2. cli::run 装配：JournalStore(init|open) + Scheduler + 事件总线 + 事件→RunStore 落盘任务
3. Runtime::new 注册 SDK 原语到 Lua 沙箱（屏蔽 io/os/fs/network）
4. spawn_blocking 驱动 Runtime::execute(script)
5. 脚本调用 agent()/parallel()/pipeline()/converge()
6. SDK 桥接层 → build_task → journal cache 检查 → handle.block_on(Scheduler.run_agent)
7. Scheduler → 选 Backend → 执行（重试/超时/取消/schema 校验）→ AgentResult
8. journal.record_result 回写（cache key 索引）；AgentEvent 广播
9. 脚本调用 report(value) → 设置最终输出
10. emit RunDone → 持久化 checkpoint.json + events.jsonl
```

逐模块的内部数据流见各自专篇。

## 依赖关系

```
                    cli ──► planner ──► runtime ──► core ◄── adapters
                     └────────────────────────────► core ◄── mcp
```

`core` 无上游依赖；其余模块都依赖 `core` 的合约。第三方关键依赖：

| crate | 用途 |
|-------|------|
| `mlua 0.10`（lua54 + vendored + async + serialize + send） | Lua 5.4 VM |
| `tokio 1`（full） | 异步运行时 |
| `agent-client-protocol 0.11.1` | ACP schema 与连接原语 |
| `dashmap 6` · `tokio-util 0.7` | 并发 map · CancellationToken |
| `blake3 1` · `unicode-normalization 0.1` | 确定性缓存键 |
| `jsonschema 0.18` | 结构化输出校验 |
| `clap 4` · `serde`/`serde_json` · `uuid`(v7) · `chrono` · `anyhow`/`thiserror` | CLI · 序列化 · id · 时间 · 错误 |

## 设计决策（贯穿全局）

- **Lua（mlua）而非 JS**：原生集成、轻量、沙箱可控。详见 [runtime.md](./architecture/runtime.md)。
- **调度集中、后端可插拔**：并发/配额/重试/取消统一在 scheduler；后端只管一次 prompt→result。
- **事件总线作为单一事实源**：避免多套状态。
- **journal 始终开启**：任意 run 可 `--resume`；事件落盘与 journal 共用同一 `RunStore` 防 split-brain。详见 [cli.md](./architecture/cli.md)。

### `parallel` vs `pipeline`

- `parallel()`：**栅栏**，所有 item 全部完成才返回，结果保序。
- `pipeline()`：多阶段处理。⚠️ 当前实现是**逐阶段栅栏 + handler 内联**（受单线程 Lua VM 约束），非真流式——详见 [runtime.md §6](./architecture/runtime.md#6-pipeline文档语义-vs-实际实现重要)。

### 收敛策略（converge）

producer 生成 findings → adversary 投票 → 存活 finding 进入下一轮，直到收敛或达 `max_rounds`。默认 `vote_threshold=0.7`、`max_rounds=3`、producers/adversaries 各 1。投票阈值的边界行为详见 [runtime.md §7](./architecture/runtime.md#7converge对抗性验证算法)。

## 已知现状与局限（v0.1）

各模块专篇的"当前状态与局限"小节有完整清单，全局要点：

- **checkpoint 非原子写入**（`fs::write` 全量重写，非 temp+rename）——见 core.md §5.1。
- **ExecLimits 已定义未强制**——见 [runtime.md §5](./architecture/runtime.md)。
- **pipeline 非真流式**——见 [runtime.md §6](./architecture/runtime.md)。
- **MCP 数据面已建未联**：agent 尚未连接 MCP server，findings 走文本回退——见 [mcp.md §6](./architecture/mcp.md)。

## 另见

- [architecture/](./architecture/) — 各模块详细架构（技术动机、关键算法、实现细节，code-accurate）
- [Dynamic Workflow 指南](./dynamic-workflow-guide.md) — 范式直觉、运转机制、Claude Code vs Maestro 对比
- [Lua Workflow 编写指南](./workflow-authoring-guide.md) — 面向开发者的实践方法论（任务分解、架构注释、错误处理、对抗验证）
- [Lua Workflow 技术规范](./dev/lua-workflow-spec.md) — 文件格式、沙箱模型、验证规则、执行生命周期
- [Lua SDK 参考](./sdk-reference.md) — 10 个原语的参数与示例
- [路线图](./roadmap.md) · [P1/P2 路线图](./roadmap-p1-p2.md) — 实施计划
- [设计文档（design/）](./design/) — P0 实现设计（[p0-acp-backend](./design/p0-acp-backend.md) / [p0-planner-resume](./design/p0-planner-resume.md)）+ 集成测试
- [归档（archive/）](./archive/) — v0.1 各模块代码设计稿 + technical-guide（均已被 architecture/ 取代，仅存历史动机）

> 注：逐模块技术详解的唯一权威源是 [architecture/](./architecture/)；原 `technical-guide.md` 与 v0.1 `design/` 代码设计稿已归档至 [archive/](./archive/)。
