# Claude Dynamic Workflows 差异分析

> 研究日期：2026-06-03（更新：2026-06-04，反映实际实现状态）
> 项目：Maestro (dynamicworkflow/maestro/)
> 参考：Claude Code Dynamic Workflows (2026-05-28 发布) + Pi fork (2026-06 扩展实现)

---

## 一、Claude Code Dynamic Workflows 核心特性

### 1.1 核心原语（API Primitives）

| 原语 | 类型 | 说明 |
|------|------|------|
| `agent(prompt, opts?)` | 基础单元 | 运行单个 subagent，可指定 model/schema/label/phase |
| `parallel(tasks)` | 栅栏（Barrier） | 等待所有任务完成才继续，适合需要全部结果的场景 |
| `pipeline(items, mapFn)` | **非栅栏（Streaming）** | 任务流式通过各阶段，不同 item 可在不同阶段并发执行 |
| `phase(name)` | 组织单元 | 将工作分组到进度 UI 阶段 |
| `log(msg)` | 状态更新 | 输出高层状态信息 |
| `budget` | 资源控制 | 共享 token 预算，可中途读取 `budget.spent()` 调整深度 |
| `workflow(name, args)` | 嵌套调用 | 将另一个 workflow 作为子步骤调用 |
| `args` | 参数注入 | workflow 参数化，接收 JSON 值 |

### 1.2 高级特性

| 特性 | 描述 |
|------|------|
| **Structured Output** | `schema` 参数强制 subagent 调用 `StructuredOutput` 工具，JSON Schema 验证在工具层完成，无需 `JSON.parse` |
| **收敛驱动迭代** | 对抗性验证：producer 生成 findings → adversary 反驳 → 投票 → surviving findings 进入下一轮，直到收敛 |
| **进度持久化（Journal）** | 运行中保存进度（已完成 agents 返回缓存结果），中断后可从断点恢复 |
| **Token 预算感知** | `budget.spent()` 可在运行中途读取，脚本据此动态调整执行深度 |
| **模型路由** | 支持 per-agent / per-phase 模型指定（Pi fork 扩展） |
| **并发限制** | 每 workflow 最多 `min(16, cpu_cores - 2)` 并发，总量上限 1,000 agents |
| **确定性编排** | 循环/分支在脚本变量中，不在 model context 中，只有 `agent()` 调用消耗 token |
| **内置 workflow** | `/deep-research` — 多角度网络搜索、交叉验证、投票综合报告 |
| **触发方式** | 1) 提示词包含 "workflow"；2) `/workflows new`；3) `ultracode`（自动决策） |
| **进度查看** | `/workflows` 列出运行中/已完成 runs，交互式 TUI 查看 phase/agent/token 统计 |

### 1.3 执行模型对比

```
标准 Subagent                          Dynamic Workflow
─────────────────────────────────────────────────────────────────────
Claude 是编排器                         脚本是编排器
每步结果回到 Claude context            只有最终结果回到 Claude context
上下文随每个 turn 累积                 中间状态在脚本变量中
并发受限于人工监控（4-8 个）            并发由 runtime 控制（最多 16 个）
不可恢复                               可从 journal 恢复（已完成 agents 缓存）
```

### 1.4 Pi fork 扩展（第三方实现）

| 能力 | 上游 Claude | Pi fork |
|------|:-----------:|:-------:|
| Core `agent`/`parallel`/`pipeline` | ✅ | ✅ |
| Structured JSON-Schema output | ✅ | ✅ |
| Token & cost accounting | 估算 | ✅ 真实值（从 SDK session 读取） |
| Per-agent / per-phase model routing | prose only | ✅ 实际切换模型 |
| `/workflows` 命令 + 交互式 TUI | — | ✅ |
| **Resume** | — | ✅ journal 回放 |
| **Git worktree 隔离** | — | ✅ 真实 worktrees + 自动清理 |
| **嵌套 `workflow()`** | — | ✅ 共享全局 cap |
| **非阻塞后台运行 + auto-continue** | — | ✅ |

---

## 二、Maestro 与 Claude 的差距分析（基于源码审计）

> 核心原语对照表、系统能力对比、剩余差距详见
> [`do../archive/v0.1-status-snapshot.md`](../archive/v0.1-status-snapshot.md) §三「对标分析」和 §六「风险矩阵」。

### 2.1 关键设计决策

#### 脚本语言：Lua vs JavaScript

**当前选择**：Lua
**理由**：Maestro 用 Lua 作为编排语言，通过 mlua 在 Rust 中原生集成，更轻量。
**风险**：Claude Code Dynamic Workflows 用 JS，生态工具（linter、IDE support）更丰富。
**建议**：保持 Lua，提供完整原语覆盖。

#### Pipeline vs Parallel

Claude 的设计原则：**默认用 `pipeline()`，`parallel()` 仅当需要全部结果才用**。
Maestro 已实现两者：`pipeline()` 非栅栏流式，`parallel()` 保持 barrier。

#### 收敛判断策略

- 投票阈值：≥70% → survive
- 最大轮次：3 轮
- 稳定检测：连续两轮 findings 集合不变 → 收敛

---

## 三、参考资源

### Claude 官方文档
- [Orchestrate subagents at scale with dynamic workflows](https://code.claude.com/docs/en/workflows) — 官方文档
- [Introducing dynamic workflows](https://claude.com/blog/introducing-dynamic-workflows-in-claude-code) — 官方博客
- [A harness for every task: dynamic workflows in Claude Code](https://claude.com/blog/a-harness-for-every-task-dynamic-workflows-in-claude-code) — 技术博客

### 社区参考
- [ray-amjad/claude-code-workflow-creator](https://github.com/ray-amjad/claude-code-workflow-creator) — Workflow tool 技能
- [QuintinShaw/pi-dynamic-workflows](https://github.com/QuintinShaw/pi-dynamic-workflows) — Pi fork
- [alexop.dev: Claude Code Workflows](https://alexop.dev/posts/claude-code-workflows-deterministic-orchestration/) — 确定性编排博客
- [DEV Community: Opus 4.8 Dynamic Workflows](https://dev.to/layzerzero105/opus-48-ships-dynamic-workflows-hundreds-of-parallel-subagents-per-session-read-this-before-you-4178) — 技术深度分析
