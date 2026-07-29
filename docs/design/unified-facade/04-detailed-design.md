# 4. 详细设计

## 4.1 Phase 1: 模块下沉到 `luft-core`

将 4 个纯底层模块从 `luft-service` 移到 `luft-core`：

| 文件 | 从 | 到 | 说明 |
|------|----|----|------|
| `json_to_lua.rs` | `luft-service/src/` | `luft-core/src/json_to_lua.rs` | 纯 `serde_json` 工具函数 |
| `params.rs` | `luft-service/src/` | `luft-core/src/params.rs` | 解析 + 校验工具，依赖 `json_to_lua` |
| `query.rs` | `luft-service/src/` | `luft-core/src/query.rs` | 磁盘查询函数，依赖 `luft-core` 类型 |
| `phases.rs` | `luft-service/src/` | `luft-core/src/phases.rs` | Phase 树构建，依赖 `luft-core` + `luft-planner` |

**受影响的导入路径更新**：

| 旧路径 | 新路径 |
|--------|--------|
| `luft_service::query::StatusOutput` | `luft_core::query::StatusOutput` |
| `luft_service::query::ReportStatus` | `luft_core::query::ReportStatus` |
| `luft_service::query::get_status` | `luft_core::query::get_status` |
| `luft_service::params::inject_args_globals` | `luft_core::params::inject_args_globals` |
| `luft_service::params::EventsFilter` | `luft_core::params::EventsFilter` |
| `luft_service::phases::*` | `luft_core::phases::*` |
| `luft_service::json_to_lua::*` | `luft_core::json_to_lua::*` |

`luft-core/Cargo.toml` 新增依赖（如果没有的话）：

```toml
luft-planner = { path = "../luft-planner" }   # for phases.rs
chrono = "0.4"                                  # for query.rs / phases.rs
```

## 4.2 Phase 2: `run.rs` 合并到 `luft`

`run.rs` 依赖 `luft-runtime` + `luft-storage` + `luft-planner`，属于引擎编排逻辑。合并到 `luft` crate：

| 文件 | 从 | 到 |
|------|----|----|
| `run.rs` | `luft-service/src/run.rs` | `luft/src/run.rs` |

**受影响的导入路径更新**：

| 旧路径 | 新路径 |
|--------|--------|
| `luft_service::run::execute` | `luft::run::execute` |
| `luft_service::run::resolve_fresh` | `luft::run::resolve_fresh` |
| `luft_service::run::prepare` | `luft::run::prepare` |

## 4.3 Phase 3: `luft` 解除对 `luft-service` 的依赖

Phase 1 + 2 完成后，`luft` 不再引用任何 `luft_service::*` 路径。

- `luft/Cargo.toml` 移除 `luft-service` 依赖
- `luft/src/lib.rs` 移除 `pub use luft_service as service;`

## 4.4 Phase 4: `WorkflowServiceImpl` 归入 `luft-service`

依赖方向修复后，`luft-service` 可以依赖 `luft`。

`luft-service/Cargo.toml` 新增：

```toml
luft        = { path = "../luft", version = "0.3.4" }
luft-runtime = { path = "../luft-runtime", version = "0.3.4" }   # validate_workflow (可能已有)
```

`luft-service/src/service.rs` 从纯 trait 定义扩展为 trait + impl：

```rust
//! WorkflowService trait + WorkflowServiceImpl

use crate::error::ServiceError;
use crate::request::*;
use crate::response::*;
use luft::Luft;
use luft::run::*;          // 原 luft-service::run，现在在 luft
use luft_core::params;     // 下沉后的 params
use luft_core::query;      // 下沉后的 query
use luft_core::query::StatusOutput;
use luft_runtime::validate_workflow;
use luft_core::contract::event::{AgentEvent, LogLevel};
use luft_core::contract::ids::AgentId;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ── Trait ──

pub trait WorkflowService: Send + Sync {
    async fn start_workflow(
        &self,
        req: ExecuteWorkflowRequest,
    ) -> Result<(ExecuteWorkflowResponse, luft::RunHandle), ServiceError>;

    fn list_files(&self) -> Result<Vec<WorkflowFile>, ServiceError>;
    fn list_runs(&self, req: ListRunsRequest) -> Result<ListRunsResponse, ServiceError>;
    fn get_run_status(&self, req: GetRunStatusRequest) -> Result<RunStatusResponse, ServiceError>;
    fn get_run_events(&self, req: GetRunEventsRequest) -> Result<RunEventsResponse, ServiceError>;
    fn cancel_run(&self, req: CancelRunRequest) -> Result<CancelRunResponse, ServiceError>;
}

// ── Implementation ──

pub struct WorkflowServiceImpl {
    luft: Luft,
    search_dirs: Vec<PathBuf>,
}

impl WorkflowServiceImpl {
    pub fn new(luft: Luft, search_dirs: Vec<PathBuf>) -> Self { ... }
    pub fn luft(&self) -> &Luft { &self.luft }
}

impl WorkflowService for WorkflowServiceImpl {
    // 全部业务逻辑从当前 luft-mcp/src/server_rmcp.rs 原样搬入
    // ...
}

// 私有 helpers 一并搬入
```

`start_workflow` 在 trait 上（不再需要区分 trait / 非 trait），三个调用者统一通过 `Arc<dyn WorkflowService>` 调用。

## 4.5 Phase 5: `luft-mcp` 瘦身

`server_rmcp.rs` 删除 `WorkflowServiceImpl` 及所有业务逻辑/helpers/tests，退化为纯 Facade：

```rust
#[derive(Clone)]
pub struct LuftMcpServer {
    service: Arc<dyn luft_service::WorkflowService>,
    tool_router: ToolRouter<Self>,
    client_name: Arc<OnceLock<String>>,
}

#[tool_router]
impl LuftMcpServer {
    #[tool(description = "...")]
    async fn execute_workflow(&self, Parameters(req): Parameters<ExecuteWorkflowRequest>)
        -> Result<String, String>
    {
        let (resp, _handle) = self.service.start_workflow(req).await.map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }
    // 其余 5 个 tool 同构
}
```

## 4.6 Phase 6: `luft-cli` 改造（渐进式）

查询类 command（`status.rs` / `list.rs`）改为通过 `WorkflowService` 调用。

`run.rs` 改用 `start_workflow()` + 保留 Presentation 逻辑（交互确认、auto-fix retry、TUI 渲染、artifact 输出）。

## 4.7 Phase 7: Loom `tool-workflow` 改造

`service.rs` / `runtime.rs` 替换为 `Arc<dyn WorkflowService>` 薄包装。7 个 tool 委托 Service。

保留不变：`backend.rs`、`event_bridge.rs`、`lib.rs` 的 `register_workflow_tools()`。
