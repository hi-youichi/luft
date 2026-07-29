# luft-service 领域服务层

> **状态**: 设计稿
> **目标**: 将 `luft-mcp` 的业务逻辑下沉为纯 Rust 类型的领域 Service，MCP 层退化为薄 Facade。
>
> **前提**: `luft-service` 当前因 `luft-planner` 的 `PlanMeta` / `PlannerConfig` 在 working tree 中被删除而编译不过（预存在问题）。实施前需先恢复 `luft-planner` 或修复 `run.rs` / `phases.rs` 中的引用。

---

## 1. 背景与问题

### 1.1 当前架构

```
rmcp stdio transport
  → LuftMcpServer #[tool_router]      ← 参数解析 + 校验 + 业务逻辑 + JSON 格式化
    → self.luft.*                     ← Luft 核心编排器（底层）
```

`server_rmcp.rs` 同时承担了三个职责：
1. **Transport handler** — rmcp `#[tool]` 宏、`#[tool_handler]` trait impl
2. **参数组装 / 校验** — `parse_concurrency`、`resolve_script_source`、`parse_status_filter` 等
3. **业务逻辑** — `derive_phases`、`derive_report_and_error`、`build_rich_status`、validation、args injection、resume 判断

`luft-service` crate 已有零散的模块（`query.rs`、`run.rs`、`phases.rs`、`params.rs`、`json_to_lua.rs`），但它们是散落的函数集合，没有组织成领域 Service。

### 1.2 核心问题

- **MCP 层过厚** — 600+ 行的 `server_rmcp.rs` 混合了三层逻辑，任何业务变更都要改 transport 层
- **不可复用** — 如果未来加 HTTP API、CLI 子命令、或 gRPC 入口，这些业务逻辑需要重新实现
- **参数解析重复** — `server_rmcp.rs` 中有一套 `parse_concurrency` / `parse_list_runs_limit` / `parse_status_filter`，`luft-service/src/params.rs` 中有另一套（签名不同但逻辑相同）
- **返回类型无约束** — tool 方法返回 `Result<String, String>`（序列化后的 JSON 字符串），没有类型化的 Response

---

## 2. 目标架构

```
┌─────────────────────────────────────────────────────┐
│ Layer 1: Transport                                  │
│  rmcp #[tool_router] / 未来的 HTTP / CLI            │
│  接收 Value, 调用 Service, 序列化 Response           │
├─────────────────────────────────────────────────────┤
│ Layer 2: Service (领域逻辑)                          │
│  WorkflowService trait + impl                       │
│  纯 Rust 类型入参/出参, 不依赖任何 transport         │
├─────────────────────────────────────────────────────┤
│ Layer 3: Luft Core (编排引擎)                       │
│  start_script / status / events / cancel / list     │
│  保持不动                                            │
└─────────────────────────────────────────────────────┘
```

### 2.1 设计原则

| 原则 | 说明 |
|------|------|
| Service 接口用纯 Rust 类型 | `Request` / `Response` struct，不出现 `serde_json::Value` |
| Request 可被 serde 反序列化 | `#[derive(Deserialize, JsonSchema)]`，rmcp `Parameters<T>` 直接复用，无需手写 assembler |
| Service 不感知 transport | 不引用 `rmcp`、不引用 `axum`、不引用 `clap` |
| Facade 极薄 | Transport 层只做 3 件事：反序列化 Request → 调 Service → 序列化 Response |
| Luft core 不动 | Service 层封装和组合 Luft，不替代它 |

---

## 3. 详细设计

### 3.1 luft-service crate 新结构

```
luft-service/
├── lib.rs           # pub re-exports
├── service.rs       # WorkflowService trait + WorkflowServiceImpl<Luft>
├── request.rs       # 领域请求类型
├── response.rs      # 领域响应类型
├── error.rs         # ServiceError enum
├── params.rs        # (保留) 底层参数解析工具函数
├── json_to_lua.rs   # (保留) JSON → Lua 序列化
├── query.rs         # (保留) 底层磁盘查询函数
├── run.rs           # (保留) 底层运行生命周期函数
└── phases.rs        # (保留) Phase 树构建
```

新增 `service.rs`、`request.rs`、`response.rs`、`error.rs` 四个文件。
现有模块不动，成为 Service 实现的内部依赖。

### 3.2 Request 类型 (`request.rs`)

每个操作一个 Request struct，`#[derive(Deserialize, JsonSchema)]`。
校验逻辑放在 `validate()` 方法或 `TryFrom` 中，而非散落在 Service 方法里。

```rust
use serde::Deserialize;
use rmcp::schemars;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExecuteWorkflowRequest {
    pub script: Option<String>,
    pub path: Option<String>,
    pub resume_from_id: Option<String>,
    pub args: Option<serde_json::Value>,
    pub concurrency: Option<u64>,
}

impl ExecuteWorkflowRequest {
    /// 校验业务规则（互斥、范围等）。
    pub fn validate(&self) -> Result<(), ServiceError> { ... }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListRunsRequest {
    pub limit: Option<u64>,
    pub cursor: Option<String>,
    pub status_filter: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetRunStatusRequest {
    pub run_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetRunEventsRequest {
    pub run_id: String,
    pub since_event_id: Option<String>,
    pub offset: Option<u64>,
    pub events_limit: Option<u64>,
    pub types: Option<Vec<String>>,
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CancelRunRequest {
    pub run_id: String,
}
```

> **注意**: `ExecuteWorkflowRequest` 派生了 `schemars::JsonSchema`。已验证 rmcp 0.6 从 `rmcp::schemars` re-export `schemars` crate，`luft-service` 只需依赖 `rmcp`（features = `["macros"]`），不需要单独依赖 `schemars`。

### 3.3 Response 类型 (`response.rs`)

纯领域模型，`#[derive(Serialize)]`，不包含 transport 层信息。

```rust
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct ExecuteWorkflowResponse {
    pub run_id: String,
    pub status: String,
    pub resumed_from: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowFile {
    pub name: String,
    pub path: String,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub task: String,
    pub status: String,
    pub total_tokens: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct ListRunsResponse {
    pub runs: Vec<RunSummary>,
    pub count: usize,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Serialize)]
pub struct PhaseAgentView {
    pub short_id: String,
    pub status: String,
    pub tokens: Option<u64>,
    pub findings: usize,
    pub last_message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PhaseView {
    pub phase_id: u32,
    pub label: String,
    pub status: String,        // "running" | "completed"
    pub planned: Option<usize>,
    pub ok: usize,
    pub failed: usize,
    pub agents: Vec<PhaseAgentView>,
}

#[derive(Debug, Serialize)]
pub struct RunStatusResponse {
    // — 基础字段 (来自 StatusOutput) —
    pub run_id: String,
    pub task: String,
    pub status: String,
    pub total_tokens: u64,
    pub created_at: String,
    pub updated_at: String,

    // — 富状态 (由事件流推导) —
    pub total_phases: usize,
    pub phases: Vec<PhaseView>,
    pub report: Value,
    pub error: Value,
}

#[derive(Debug, Serialize)]
pub struct RunEventsResponse {
    pub events: Vec<Value>,
    pub offset: u64,
    pub events_limit: u64,
    pub total_matching: u64,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CancelRunResponse {
    pub run_id: String,
    pub result: String,          // "cancelling" | "not_found_or_terminal"
    pub note: Option<String>,
}
```

### 3.4 Error 类型 (`error.rs`)

```rust
#[derive(Debug, Serialize)]
pub struct RunStatusResponse {
    // — 基础字段 (来自 StatusOutput) —
    pub run_id: String,
    pub run_dir: String,
    pub task: String,
    pub status: String,
    pub current_phase: u32,
    pub completed_phases: usize,
    pub total_started: usize,
    pub completed_agents: usize,
    pub running_agents: usize,
    pub total_tokens: u64,
    pub created_at: String,
    pub updated_at: String,

    // — 富状态 (由事件流推导) —
    pub total_phases: usize,
    pub phases: Vec<PhaseView>,
    pub report: Value,
    pub error: Value,
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidParam(msg) => write!(f, "{msg}"),
            Self::NotFound(id) => write!(f, "run not found: {id}"),
            Self::Internal(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<anyhow::Error> for ServiceError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e.to_string())
    }
}
```

### 3.5 Service Trait (`service.rs`)

```rust
pub trait WorkflowService: Send + Sync {
    async fn execute_workflow(
        &self,
        req: ExecuteWorkflowRequest,
    ) -> Result<ExecuteWorkflowResponse, ServiceError>;

    fn list_files(&self) -> Result<Vec<WorkflowFile>, ServiceError>;

    fn list_runs(
        &self,
        req: ListRunsRequest,
    ) -> Result<ListRunsResponse, ServiceError>;

    fn get_run_status(
        &self,
        req: GetRunStatusRequest,
    ) -> Result<RunStatusResponse, ServiceError>;

    fn get_run_events(
        &self,
        req: GetRunEventsRequest,
    ) -> Result<RunEventsResponse, ServiceError>;

    fn cancel_run(
        &self,
        req: CancelRunRequest,
    ) -> Result<CancelRunResponse, ServiceError>;
}
```

### 3.6 Service 实现

```rust
pub struct WorkflowServiceImpl {
    luft: Arc<Luft>,       // Arc<Luft> 而非 Luft，因为 Luft 不是 Clone
    search_dirs: Vec<PathBuf>,
}

impl WorkflowServiceImpl {
    pub fn new(luft: Luft, search_dirs: Vec<PathBuf>) -> Self { ... }
}

impl WorkflowService for WorkflowServiceImpl {
    async fn execute_workflow(&self, req: ExecuteWorkflowRequest) -> Result<...> {
        req.validate()?;
        // — resume 分支 —
        if let Some(id) = req.resume_from_id.as_deref().filter(|s| !s.is_empty()) {
            let handle = self.luft.start_resume(id).await?;
            // 后台异步等待 run 完成并写入 instance.json（终态缓存）
            let luft = self.luft.clone();
            tokio::spawn(async move {
                let _ = handle.join().await;
                write_instance_json(handle.run_dir_name(), luft.base_dir());
            });
            return Ok(ExecuteWorkflowResponse { run_id: ..., status: "running", resumed_from: Some(id) });
        }
        // — 正常执行分支 —
        let script = self.resolve_script_source(&req)?;
        let script = inject_args_globals(&script, req.args.as_ref());
        let validation = validate_workflow(&script)?;
        if !validation.is_valid() { return Err(ServiceError::InvalidParam(...)); }

        // — 并发覆盖 —
        // with_concurrency 返回 owned Luft，需要局部变量保持生命周期。
        // 对于 async 方法，局部变量在 .await 期间存活，无问题。
        let scoped_luft;
        let luft: &Luft = match req.concurrency.and_then(validate_concurrency) {
            Some(n) => { scoped_luft = self.luft.with_concurrency(n); &scoped_luft }
            None => &*self.luft,
        };

        let handle = luft.start_script(&script).await?;
        let luft = self.luft.clone();
        tokio::spawn(async move {
            let _ = handle.join().await;
            write_instance_json(handle.run_dir_name(), luft.base_dir());
        });
        Ok(ExecuteWorkflowResponse { run_id: ..., status: "running", resumed_from: None })
    }
    // ... 其余方法同理
}

/// 异步写入 instance.json — get_run_status 的终态优先来源。
fn write_instance_json(run_dir: &str, base_dir: &Path) { ... }
```
```

**从 `server_rmcp.rs` 下沉到 Service 的逻辑：**

| 函数 | 来源 | 去向 |
|------|------|------|
| `resolve_script_source` | `server_rmcp.rs` | Service 私有方法 |
| `parse_concurrency` | `server_rmcp.rs` + `params.rs` | `request.rs` 的 `validate()` |
| `parse_list_runs_limit` | `server_rmcp.rs` | `request.rs` 的 `validate()` |
| `parse_status_filter` | `server_rmcp.rs` | `request.rs` 的 `validate()` |
| `filter_events_since` | `server_rmcp.rs` | Service 私有方法 |
| `derive_phases` | `server_rmcp.rs` | Service 私有方法 |
| `derive_report_and_error` | `server_rmcp.rs` | Service 私有方法 |
| `build_rich_status` | `server_rmcp.rs` | Service 私有方法 |
| `summarize_output` | `server_rmcp.rs` | Service 私有方法 |
| `is_terminal_status` | `server_rmcp.rs` | Service 私有方法 |
| `inject_args_globals` | `params.rs` | 复用现有 `params.rs` |
| `paginate` / `paginate_cursor` | `params.rs` | 复用现有 `params.rs` |
| `write_instance_json` | 原 `tools.rs`（当前丢失） | Service 私有方法（需恢复） |

### 3.7 MCP Facade 层（`server_rmcp.rs` 瘦身后）

```rust
#[derive(Clone)]
pub struct LuftMcpServer {
    service: Arc<dyn WorkflowService>,
    tool_router: ToolRouter<Self>,
    client_name: Arc<OnceLock<String>>,
}

#[tool_router]
impl LuftMcpServer {
    #[tool(description = "...")]
    async fn execute_workflow(
        &self,
        Parameters(req): Parameters<ExecuteWorkflowRequest>,
    ) -> Result<String, String> {
        let resp = self.service.execute_workflow(req).await.map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }

    #[tool(description = "...")]
    fn list_files(&self) -> Result<String, String> {
        let resp = self.service.list_files().map_err(|e| e.to_string())?;
        serde_json::to_string(&resp).map_err(|e| e.to_string())
    }

    // ... 其余 4 个 tool，每个 3 行
}
```

**瘦身后效果：**
- `server_rmcp.rs` 从 ~600 行 → ~120 行
- 所有参数解析、校验、业务逻辑、格式化全部消失
- `LuftMcpServer` 不再直接持有 `Luft`，改为持有 `Arc<dyn WorkflowService>`
- `ServerHandler` / resources impl 不变（那部分是 rmcp transport 逻辑）

---

## 4. 实现步骤

| 步骤 | 内容 | 涉及文件 |
|------|------|---------|
| 0 | **前置**：修复 `luft-service` 编译（`luft-planner` 引用） | `luft-planner/src/lib.rs` 或 `luft-service/src/{run,phases}.rs` |
| 1 | 新建 `error.rs` — `ServiceError` | `luft-service/src/error.rs` |
| 2 | 新建 `request.rs` — 6 个 Request struct + `validate()` | `luft-service/src/request.rs` |
| 3 | 新建 `response.rs` — 6 个 Response struct + 子类型 | `luft-service/src/response.rs` |
| 4 | 新建 `service.rs` — `WorkflowService` trait + `WorkflowServiceImpl`，业务逻辑从 `server_rmcp.rs` 迁入 | `luft-service/src/service.rs` |
| 5 | 更新 `luft-service/src/lib.rs` — 导出新模块 | `luft-service/src/lib.rs` |
| 6 | 更新 `luft-service/Cargo.toml` — 加 `rmcp/schemars` + `luft-runtime` 依赖 | `luft-service/Cargo.toml` |
| 7 | 瘦身 `server_rmcp.rs` — 替换为 Service facade | `luft-mcp/src/server_rmcp.rs` |
| 8 | 迁移测试 — `server_rmcp.rs` 的测试拆分到 `service.rs`（业务逻辑）和 `server_rmcp.rs`（client identity） | 两处 |
| 9 | 编译 + `cargo nextest run -p luft-mcp` | — |

---

## 5. 测试策略

### 5.1 Service 层测试（`service.rs`）

所有原有 `server_rmcp.rs` 中的业务逻辑测试迁移到 `service.rs`，用纯 Service API 调用：

```rust
#[cfg(test)]
mod tests {
    fn make_service() -> WorkflowServiceImpl { ... }

    #[tokio::test]
    async fn execute_workflow_neither_script_nor_path() {
        let svc = make_service();
        let req = ExecuteWorkflowRequest { script: None, path: None, resume_from_id: None, args: None, concurrency: None };
        let err = svc.execute_workflow(req).await.unwrap_err();
        assert!(matches!(err, ServiceError::InvalidParam(_)));
    }

    #[test]
    fn derive_phases_single_phase_single_agent() { ... }  // 原有测试不变
}
```

### 5.2 Facade 层测试（`server_rmcp.rs`）

只保留 transport 特有逻辑的测试：

```rust
#[cfg(test)]
mod tests {
    // client_name / is_codex / OnceLock 语义 — 4 个测试
    // 不再测试业务逻辑（那些在 service.rs 中）
}
```

### 5.3 Request 校验测试（`request.rs`）

```rust
#[test]
fn concurrency_rejects_zero() {
    let req = ExecuteWorkflowRequest { concurrency: Some(0), ..default() };
    assert!(req.validate().is_err());
}
```

---

## 6. 影响范围

| Crate | 变更 |
|-------|------|
| `luft-service` | 新增 4 文件，`lib.rs` 导出更新 |
| `luft-mcp` | `server_rmcp.rs` 大幅瘦身，`lib.rs` 不变 |
| `luft` (core) | **不动** |
| `luft-cli` | 不需要改（CLI 直接用 `luft-service::query`，不经过 Service） |

### 6.1 依赖变更

`luft-service/Cargo.toml` 新增：
```toml
rmcp = { version = "0.6", default-features = false, features = ["macros"] }   # re-exports schemars
luft-runtime = { path = "../luft-runtime" }   # for validate_workflow
```

---

## 7. 不做的事情

- **不改 Luft core** — `WorkflowServiceImpl` 封装 `Luft`，不重构 `Luft` 本身
- **不改 CLI** — `luft-cli` 直接用 `query.rs` 等底层模块，不需要走 Service
- **不加 HTTP/gRPC transport** — 本次只做 MCP Facade，分层就绪后后续加新 transport 只需写新 Facade
- **不做 async trait crate** — 使用 Rust 原生 `async fn in trait`（stable since 1.75）

---

## 8. Review 修正记录

以下问题在 review 中发现并已在上述文档中修正：

| # | 问题 | 修正 |
|---|------|------|
| P0 | `Luft` 不是 `Clone`，`WorkflowServiceImpl` 持有 `Luft` 会有所有权问题 | 改为 `Arc<Luft>`；Facade 持有 `Arc<dyn WorkflowService>`，`Arc::clone` 天然满足 rmcp `Clone` 需求 |
| P1 | `write_instance_json`（run 终态缓存写入）在迁移中丢失 | §3.6 恢复 `tokio::spawn` + `write_instance_json` 逻辑，明确为 Service 层 side-effect |
| P2 | `Luft::with_concurrency` 返回 owned `Luft`，async 方法中局部变量生命周期 | §3.6 伪码增加 `scoped_luft` 局部变量说明 |
| P3 | `RunStatusResponse` 遗漏了 `StatusOutput` 的 6 个字段 | §3.3 补全 `run_dir`、`current_phase`、`completed_phases`、`total_started`、`completed_agents`、`running_agents` |
| P4 | `schemars` 依赖路径不确定 | 已验证 rmcp 0.6 re-exports `schemars`，`luft-service` 只需依赖 `rmcp` |
| P5 | `luft-service` 预存在编译错误（`luft-planner` 引用） | §0 前置步骤 + 文档头部前提条件说明 |
