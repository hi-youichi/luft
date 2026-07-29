# 6. 文件映射

| 层 | Crate | 文件 | 状态 |
|----|-------|------|------|
| **Phase 1: 下沉** | | | |
| Engine | `luft-core` | `src/json_to_lua.rs` | **新增**（从 luft-service 搬入） |
| Engine | `luft-core` | `src/params.rs` | **新增**（从 luft-service 搬入） |
| Engine | `luft-core` | `src/query.rs` | **新增**（从 luft-service 搬入） |
| Engine | `luft-core` | `src/phases.rs` | **新增**（从 luft-service 搬入） |
| Service | `luft-service` | `src/json_to_lua.rs` | **删除** |
| Service | `luft-service` | `src/params.rs` | **删除** |
| Service | `luft-service` | `src/query.rs` | **删除** |
| Service | `luft-service` | `src/phases.rs` | **删除** |
| **Phase 2: 合并** | | | |
| Engine | `luft` | `src/run.rs` | **新增**（从 luft-service 搬入） |
| Service | `luft-service` | `src/run.rs` | **删除** |
| **Phase 3: 解除依赖** | | | |
| Engine | `luft` | `src/lib.rs` | 移除 `pub use luft_service as service;` |
| Engine | `luft` | `Cargo.toml` | 移除 `luft-service` 依赖 |
| **Phase 4: Service 归位** | | | |
| Service | `luft-service` | `src/service.rs` | **重写**：trait + `WorkflowServiceImpl` + helpers |
| Service | `luft-service` | `Cargo.toml` | 新增 `luft` 依赖 |
| **Phase 5: Presentation 瘦身** | | | |
| Presentation | `luft-mcp` | `src/server_rmcp.rs` | 瘦身：删除 impl + helpers，保留 Facade |
| Presentation | `luft-cli` | `src/commands/run.rs` | 改造：改用 `start_workflow` |
| Presentation | `luft-cli` | `src/commands/status.rs` | 改造：改用 `get_run_status` |
| Presentation | `luft-cli` | `src/commands/list.rs` | 改造：改用 `list_runs` |
| **Phase 6: Loom** | | | |
| Presentation | `loom` | `agent/tool/tool-workflow/src/*.rs` | 改造：委托 `WorkflowService` |
| Presentation | `loom` | `agent/tool/tool-workflow/src/backend.rs` | 不变 |
| Presentation | `loom` | `agent/tool/tool-workflow/src/event_bridge.rs` | 不变 |
