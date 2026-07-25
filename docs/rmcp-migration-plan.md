# Luft MCP → RMCP 迁移开发方案

## 项目背景

将 Luft 项目现有的自定义 MCP (Model Context Protocol) 实现替换为官方 Rust RMCP SDK，以获得更好的协议兼容性、更丰富的功能和更低的维护成本。

## 当前实现分析

### 现有架构
```
crates/luft-mcp/
├── src/
│   ├── protocol.rs      # 自定义 JSON-RPC 2.0 协议实现
│   ├── resources.rs     # 资源处理 (workflow:// URI scheme)
│   ├── tools.rs         # 6 个工具处理器
│   └── server.rs        # stdio JSON-RPC 服务器
└── Cargo.toml
```

### 现有功能
- **工具**: execute_workflow, list_files, list_runs, get_run_status, get_run_events, cancel_run
- **资源**: workflow://schema, workflow://examples, workflow://example/{name}
- **传输**: stdio JSON-RPC
- **协议**: MCP 2024-11-05

### 已发现的 RMCP 迁移工作
项目中已存在部分 RMCP 迁移代码：
- `src/tools_rmcp.rs` - 使用 `#[tool]` 宏的工具定义（占位符实现）
- `src/resources_rmcp.rs` - 使用 `#[resource]` 宏的资源定义

## RMCP 一次性替换方案

### 1. 依赖更新

**目标**: 用 rmcp 替换自定义 JSON-RPC 实现

```toml
[dependencies]
rmcp = { version = "3.0.0-beta.2", features = ["server", "macros", "transport-io"] }
rmcp-macros = "3.0.0-beta.2"
schemars = "0.8"  # JSON Schema 生成
tokio = { version = "1", features = ["rt", "rt-multi-thread", "io-util"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"

[dev-dependencies]
rmcp = { version = "3.0.0-beta.2", features = ["testing"] }
tempfile = "3"
tokio-test = "0.4"
```

### 2. 文件重构计划

**删除文件**:
- `src/protocol.rs` (用 rmcp 的协议层替代)

**重构文件**:
- `src/lib.rs` - 重导出 rmcp 类型，简化 API
- `src/tools.rs` - 完善 `#[tool]` 宏实现的工具业务逻辑
- `src/resources.rs` - 完善 `#[resource]` 宏实现的资源接口
- `src/server.rs` - 使用 rmcp 的 `ServerHandler`

**新增文件**:
- `src/server_rmcp.rs` - RMCP 服务器实现
- `tests/integration/rmcp_protocol_test.rs` - RMCP 协议测试
- `tests/compatibility/client_test.rs` - 客户端兼容性测试

### 3. 核心实现变更

#### 工具系统改造

基于现有的 `tools_rmcp.rs`，完善业务逻辑：

```rust
use luft::Luft;
use rmcp::{tool, ToolResponse, ToolError};
use serde_json::Value;

#[tool(
    name = "execute_workflow",
    description = "Execute a Luft workflow, or resume a prior checkpointed run"
)]
pub async fn execute_workflow(
    #[tool(description = "Inline Lua workflow script")] 
    script: Option<String>,
    #[tool(description = "Path to .lua file (relative to CWD)")] 
    path: Option<String>,
    #[tool(description = "run_id of a prior checkpointed run to resume")] 
    resume_from_id: Option<String>,
    #[tool(description = "Workflow arguments")] 
    args: Option<Value>,
    #[tool(description = "Max concurrent agents")] 
    concurrency: Option<u64>,
    luft: &Luft,  // 通过上下文注入
) -> Result<ToolResponse, ToolError> {
    // 实现现有的业务逻辑
    // 从现有的 tools.rs 中迁移验证和执行逻辑
}
```

#### 资源系统改造

基于现有的 `resources_rmcp.rs`，完善资源实现：

```rust
use rmcp::{resource, ResourceResponse, ResourceError};

#[resource(uri = "workflow://schema", name = "Workflow DSL Reference")]
pub fn schema_resource() -> Result<ResourceResponse, ResourceError> {
    Ok(ResourceResponse::text(
        luft_planner::LUA_DSL_REFERENCE.to_string(),
        "text/markdown"
    ))
}

#[resource(uri_template = "workflow://example/{name}")]
pub fn example_resource(
    name: String,
    search_dirs: Option<Vec<PathBuf>>,
) -> Result<ResourceResponse, ResourceError> {
    // 复用现有的资源查找逻辑
}
```

#### 服务器改造

```rust
use rmcp::{ServerHandler, ServerServiceExt, transport::StdioTransport};

pub struct LuftMcpServer {
    luft: Luft,
    search_dirs: Vec<PathBuf>,
}

impl ServerHandler for LuftMcpServer {
    fn serve_stdio(self) -> anyhow::Result<()> {
        let transport = StdioTransport::new();
        let service = self.build_service()?;
        service.serve(transport).await?;
        Ok(())
    }
}
```

### 4. 测试策略

#### 单元测试层（保持业务逻辑测试）

```rust
// tests/unit/tools_test.rs
#[tokio::test]
async fn execute_workflow_validation() {
    // 测试工具参数验证逻辑
    let result = validate_execute_params(&json!({
        "script": "invalid lua"
    })).await;
    assert!(result.is_err());
}

// tests/unit/resources_test.rs  
#[tokio::test]
fn parse_workflow_uri() {
    assert_eq!(WorkflowUri::parse("workflow://schema"), Some(WorkflowUri::Schema));
}
```

#### RMCP 协议层测试

```rust
// tests/integration/protocol_test.rs
use rmcp::testing::TestClient;

#[tokio::test]
async fn rmcp_protocol_compliance() {
    let server = build_test_server().await;
    let mut client = TestClient::new(server).await;
    
    let init = client.initialize().await.unwrap();
    assert_eq!(init.protocol_version, "2024-11-05");
    
    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 6);
}
```

#### 兼容性测试（确保客户端 API 不变）

```rust
// tests/compatibility/client_test.rs
#[tokio::test]
async fn backward_compatibility() {
    let server = build_test_server().await;
    
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1, 
        "method": "tools/list"
    });
    
    let response = send_raw_request(&server, request).await;
    assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 6);
}
```

#### 集成测试

```rust
// tests/integration/full_workflow_test.rs
#[tokio::test]
async fn complete_workflow_execution() {
    let server = build_test_server_with_example().await;
    let mut client = TestClient::new(server).await;
    
    client.initialize().await.unwrap();
    let tools = client.list_tools().await.unwrap();
    
    let result = client.call_tool("execute_workflow", json!({
        "path": "example.lua"
    })).await.unwrap();
    
    let run_id = result["run_id"].as_str().unwrap();
    let status = client.call_tool("get_run_status", json!({
        "run_id": run_id
    })).await.unwrap();
    
    assert!(!status["isError"].as_bool().unwrap());
}
```

### 5. 测试覆盖率目标

| 层级 | 覆盖率目标 | 测试类型 |
|------|------------|----------|
| 工具逻辑 | 95%+ | 单元测试 |
| 资源处理 | 90%+ | 单元测试 |
| RMCP 协议 | 85%+ | 集成测试 |
| 客户端兼容 | 100% | 兼容性测试 |

### 6. 实施步骤

#### 阶段 1: 依赖和基础结构
1. 更新 `Cargo.toml` 添加 RMCP 依赖
2. 保留现有代码，确保所有测试通过
3. 创建新的 RMCP 服务器框架

#### 阶段 2: 工具逻辑迁移
1. 完善 `tools_rmcp.rs` 中的业务逻辑
2. 从现有的 `tools.rs` 迁移验证和执行逻辑
3. 添加 Luft 实例的上下文注入
4. 运行工具单元测试

#### 阶段 3: 资源逻辑迁移
1. 完善 `resources_rmcp.rs` 中的资源实现
2. 确保与现有资源逻辑行为一致
3. 运行资源单元测试

#### 阶段 4: 服务器实现
1. 创建 `server_rmcp.rs` 使用 RMCP 的 `ServerHandler`
2. 实现 stdio 传输层
3. 确保协议兼容性

#### 阶段 5: 集成和测试
1. 运行所有现有测试确保兼容性
2. 添加 RMCP 协议测试
3. 添加客户端兼容性测试
4. 性能基准测试

#### 阶段 6: 清理和文档
1. 删除废弃的 `protocol.rs`
2. 更新 `lib.rs` 导出
3. 更新 API 文档
4. 清理测试代码

### 7. 关键验证点

- ✅ 所有 6 个工具的功能保持不变
- ✅ 资源 URI 解析和读取逻辑一致  
- ✅ 错误处理和错误码兼容
- ✅ 性能无明显下降（<10% 差异）
- ✅ stdio 传输协议完全兼容
- ✅ MCP 2024-11-05 协议版本支持

### 8. 风险和缓解措施

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| RMCP API 变化 | 高 | 锁定版本，关注上游变更 |
| 性能下降 | 中 | 基准测试，优化热点 |
| 客户端不兼容 | 高 | 全面的兼容性测试 |
| 功能缺失 | 中 | 详细的功能对比测试 |

### 9. 时间估算

- 阶段 1: 1-2 天
- 阶段 2: 2-3 天  
- 阶段 3: 1-2 天
- 阶段 4: 1-2 天
- 阶段 5: 2-3 天
- 阶段 6: 1 天

**总计**: 8-13 天

### 10. 成功标准

1. 所有现有测试通过
2. RMCP 协议测试通过
3. 客户端兼容性测试 100% 通过
4. 性能无明显下降
5. 代码覆盖率不降低
6. 文档更新完整

## 当前状态

- ✅ Worktree 创建完成: `worktrees/luft/feat-rmcp-integration`
- ✅ 基础 RMCP 文件已存在: `tools_rmcp.rs`, `resources_rmcp.rs`
- ⏳ 需要完善业务逻辑实现
- ⏳ 需要创建 RMCP 服务器适配层
- ⏳ 需要添加完整的测试覆盖

## 下一步行动

1. 确认开发方案和优先级
2. 开始阶段 1: 依赖更新和基础结构
3. 逐步完成各个阶段的实施
4. 持续运行测试确保质量

---

**文档创建时间**: 2025-08-19  
**Worktree**: `C:\Users\heycj\dev\worktrees\luft\feat-rmcp-integration`  
**分支**: `worktree-feat-rmcp-integration`