# 统一 Facade 层：luft-cli / luft-mcp / Loom

> **状态**: 设计稿（v2 — 修正依赖倒置）
> **目标**: 修复 `luft → luft-service` 依赖倒置，将 `WorkflowServiceImpl` 归入 `luft-service`，让 CLI、MCP、Loom 共用同一个 Service facade。

**概述**：当前 `luft`（引擎）反向依赖 `luft-service`（应为上层），根因是 `query.rs` / `params.rs` / `phases.rs` / `json_to_lua.rs` / `run.rs` 这些本属底层的模块被放在了 `luft-service` 中。本方案将这些模块下沉到 `luft-core` / `luft`，消除依赖倒置。此后 `luft-service` 可以正常依赖 `luft`，`WorkflowServiceImpl` 归入 `luft-service`，三个调用者（CLI / MCP / Loom）通过 `WorkflowService` trait 统一调用。涉及两个仓库（luft + loom），分 14 步实施。

## 目录

| 文件 | 内容 |
|------|------|
| [01-current-state.md](01-current-state.md) | 现状：三个调用者的架构、重复逻辑、问题 |
| [02-solution.md](02-solution.md) | 方案：根因分析、模块下沉、修复后依赖方向 |
| [03-target-architecture.md](03-target-architecture.md) | 目标架构：分层总览、crate 归属、接口契约、调用序列 |
| [04-detailed-design.md](04-detailed-design.md) | 详细设计：Phase 1-7 每步的具体变更 |
| [05-implementation-steps.md](05-implementation-steps.md) | 实现步骤：14 步任务列表 |
| [06-file-mapping.md](06-file-mapping.md) | 文件映射：每个文件的变更状态 |
| [07-scope-risks.md](07-scope-risks.md) | 不做的事情 + 风险 + v1/v2 对比 |
