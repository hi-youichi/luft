> **状态**: 待完善 — 本文由文档整理工作流自动生成。

# Maestro 路线图

> P1/P2 具体排期见 [roadmap-p1-p2.md](./roadmap-p1-p2.md)。

## P0 — 基础设施（已完成）

| 代号 | 标题 | 状态 | 详细设计 |
|------|------|------|---------|
| P0-A | OpenCode ACP 真实后端 | ✅ 已实现（2026-06-03） | [design/p0-acp-backend.md](./design/p0-acp-backend.md) |
| P0-B | Planner agent 化（NL → Lua 动态生成） | ✅ 已实现（2026-06-03） | [design/p0-planner-resume.md](./design/p0-planner-resume.md) |
| P0-C | Resume（journal 缓存式恢复） | ✅ 已实现（2026-06-03） | [design/p0-planner-resume.md](./design/p0-planner-resume.md) |

P0 交付了核心编排能力：通过 ACP 协议接入真实 LLM 后端、NL 动态生成编排脚本、基于 journal 的缓存恢复。

## P1 — 增强稳定与可观测性（规划中）

预期方向：

- **ExecLimits 强制落地** — runtime 中限时/限 token，避免 agent 失控（见 [architecture/runtime.md](./architecture/runtime.md) §5）
- **Pipeline 真流式** — 解除逐阶段栅栏限制，支持阶段间流式传递（见 [architecture/runtime.md](./architecture/runtime.md) §6）
- **MCP 数据面接通** — agent 真实连接 MCP server，findings 结构化上报而非文本回退（见 [architecture/mcp.md](./architecture/mcp.md) §6）
- **Checkpoint 原子写入** — temp + rename 替代全量 `fs::write`（见 [architecture/core.md](./architecture/core.md) §5.1）
- **进度上报与取消** — `RunContext` 中 `CancellationToken` + `EventSender` 全线打通
- **集成测试套件** — 覆盖端到端 workflow 场景

## P2 — 生态与高级特性（远期）

预期方向：

- **多模型混合编排** — 单次 workflow 中按阶段选择不同 backend
- **Human-in-the-loop 审批流** — 策略化审批、断点续批
- **可视化 Dashboard** — 事件流实时展示 + 历史 run 查询
- **插件系统** — 自定义 SDK 原语扩展
- **分布式调度** — 跨节点 agent 编排

---

## 另见

- [architecture.md](./architecture.md) — 架构总览与已知局限
- [P1/P2 路线图（roadmap-p1-p2.md）](./roadmap-p1-p2.md) — 详细排期与优先级
- [design/](./design/) — 各阶段实现设计文档
