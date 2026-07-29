# 5. 实现步骤

| 步骤 | 内容 | 涉及项目 |
|------|------|---------|
| **Phase 1: 模块下沉** | | |
| 1 | `json_to_lua.rs` → `luft-core`，更新导入 | luft |
| 2 | `params.rs` → `luft-core`，更新导入 | luft |
| 3 | `query.rs` → `luft-core`，更新导入 | luft |
| 4 | `phases.rs` → `luft-core`，更新导入 | luft |
| 5 | `cargo nextest run -p luft-core -p luft-service` | luft |
| **Phase 2: run.rs 合并** | | |
| 6 | `run.rs` → `luft`，更新导入 | luft |
| 7 | `cargo nextest run -p luft` | luft |
| **Phase 3: 解除依赖** | | |
| 8 | `luft/Cargo.toml` 移除 `luft-service` 依赖；`luft/src/lib.rs` 移除 re-export | luft |
| 9 | `cargo build --workspace` | luft |
| **Phase 4: Service 归位** | | |
| 10 | `luft-service/Cargo.toml` 新增 `luft` 依赖；`service.rs` 扩展为 trait + impl + helpers；从 `luft-mcp` 搬入业务逻辑 + 测试 | luft |
| 11 | `cargo nextest run -p luft-service` | luft |
| **Phase 5: Presentation 瘦身** | | |
| 12 | `luft-mcp/server_rmcp.rs` 改为纯 Facade；`luft-cli` 的 `status.rs` / `list.rs` / `run.rs` 改用 Service | luft |
| 13 | `cargo nextest run -p luft-mcp -p luft-cli` | luft |
| **Phase 6: Loom 改造** | | |
| 14 | Loom `tool-workflow` 替换为 `WorkflowService` 薄包装 | loom |
