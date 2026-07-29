# 7. 范围、风险与对比

## 7.1 不做的事情

- **不改 Request/Response/Error 类型** - 已在上一轮完成
- **不重构 CLI 的 Presentation 逻辑** - retry loop / TUI / artifact 输出保持原样
- **不改 Loom 的 `backend.rs` / `event_bridge.rs`** - 这些是 Loom 的 Presentation 层适配
- **不改 Loom `Tool` trait / `ToolRegistry`** - 只改 tool-workflow 内部实现

## 7.2 风险

| 风险 | 缓解 |
|------|------|
| 模块下沉涉及大量导入路径更新 | 按步骤逐个迁移，每步编译验证；用 `sed`/`grep` 批量替换 |
| `luft-core` 变重 | 可接受：query/params/phases 是纯查询/解析逻辑，与 contract 类型天然同层 |
| `luft-planner` 成为 `luft-core` 的依赖 | `phases.rs` 用 `PlanMeta`；如不愿加重 `luft-core`，可将 `phases.rs` 留在 `luft-service` |
| CLI 改造范围大（`run.rs` 有 800+ 行） | 渐进式：先改查询类 command，`run.rs` 最后改 |
| Loom `tool-workflow` 改动跨仓库 | Loom 通过 `[patch.crates-io]` 依赖本地 luft，改动即时生效；先改 luft 侧，Loom 侧后续跟进 |

## 7.3 与 v1 方案对比

| 维度 | v1（原方案） | v2（本方案） |
|------|-------------|-------------|
| `WorkflowServiceImpl` 归属 | `luft` crate | `luft-service` crate |
| 依赖方向 | 不修复倒置，利用 `luft → luft-service` 方向放 impl | 修复倒置，模块下沉到 `luft-core` / `luft` |
| `start_workflow` 在 trait 上 | 否（`RunHandle` 是 luft 类型，放不进 trait） | **是**（luft-service 可依赖 luft） |
| 调用者持有类型 | `Arc<WorkflowServiceImpl>`（具体类型） | `Arc<dyn WorkflowService>`（trait object） |
| 步骤数 | 10 | 14 |
| 根因 | 绕过 | 修复 |
