# 3. 目标架构

## 3.1 分层总览

```
+---------------------------------------------------------------+
|  Presentation Layer                                           |
|                                                               |
|  luft-cli               luft-mcp               Loom            |
|  (apps/cli)             (crates/luft-mcp)      (agent/tool-    |
|                                               tool-workflow)  |
|                                                               |
|  ┌─────────────┐       ┌─────────────┐       ┌─────────────┐  |
|  │ CLI args    │       │ rmcp stdio  │       │ Tool trait  │  |
|  │ clap parser │       │ #[tool]     │       │ ToolRegistry│  |
|  │             │       │ ServerHandler│      │ ToolSpec    │  |
|  │ ─────────── │       │ ─────────── │       │ ─────────── │  |
|  │ 交互提示     │       │ resource URI│       │ agent loop  │  |
|  │ auto-fix    │       │ JSON 序列化  │       │ event_bridge│  |
|  │ TUI 渲染    │       │             │       │ backend 适配 │  |
|  │ artifact    │       │             │       │             │  |
|  └──────┬──────┘       └──────┬──────┘       └──────┬──────┘  |
|         │                     │                     │         |
|         └─────────────────────┼─────────────────────┘         |
|                               │                               |
+-------------------------------|-------------------------------+
                                │  Arc<dyn WorkflowService>
                                v
+---------------------------------------------------------------+
|  Service Layer  [luft-service crate]                          |
|                                                               |
|  ┌─────────────────────────────────────────────────────────┐  |
|  │ WorkflowService trait          [service.rs]             │  |
|  │                                                         │  |
|  │   async fn start_workflow(req)                          │  |
|  │       -> Result<(Response, RunHandle), ServiceError>    │  |
|  │   fn list_files() -> Result<Vec<WorkflowFile>>          │  |
|  │   fn list_runs(req) -> Result<ListRunsResponse>         │  |
|  │   fn get_run_status(req) -> Result<RunStatusResponse>   │  |
|  │   fn get_run_events(req) -> Result<RunEventsResponse>   │  |
|  │   fn cancel_run(req) -> Result<CancelRunResponse>       │  |
|  │                                                         │  |
|  │   Request types    [request.rs]                         │  |
|  │   Response types   [response.rs]                        │  |
|  │   ServiceError     [error.rs]                           │  |
|  └────────────────────────┬────────────────────────────────┘  |
|                           │ impl                              |
|  ┌────────────────────────v────────────────────────────────┐  |
|  │ WorkflowServiceImpl           [service_impl.rs]         │  |
|  │                                                         │  |
|  │   pub fn new(luft, search_dirs) -> Self                 │  |
|  │   pub fn luft(&self) -> &Luft                           │  |
|  │                                                         │  |
|  │   私有 helpers:                                         │  |
|  │     resolve_script_source                               │  |
|  │     collect_workflow_files                              │  |
|  │     write_instance_json                                 │  |
|  │     derive_phases / derive_report_and_error             │  |
|  │     filter_events_since / is_terminal_status            │  |
|  └────────────────────────┬────────────────────────────────┘  |
+-------------------------------|-------------------------------+
                                │  &Luft
                                v
+---------------------------------------------------------------+
|  Engine Layer                                                 |
|                                                               |
|  ┌─────────────────────────────────────────────────────────┐  |
|  │ Luft                        [luft/builder.rs]           │  |
|  │   +- start_script / start_resume                         │  |
|  │   +- status / list / events / report / cancel            │  |
|  │   +- with_concurrency                                     │  |
|  │   +- run.rs (从 luft-service 合并)                       │  |
|  │     resolve_fresh / prepare / execute                     │  |
|  └────────────────────────┬────────────────────────────────┘  |
|                           │                                   |
|  ┌────────────────────────v────────────────────────────────┐  |
|  │ luft-core       [contract / journal / scheduler]        │  |
|  │                 [query / params / phases / json_to_lua] │  │  <- 从 luft-service 下沉
|  │ luft-runtime    [sandbox / validate_workflow]           │  |
|  │ luft-storage    [checkpoint / event store]              │  |
|  │ luft-planner    [NL -> Lua planning]                    │  |
|  └─────────────────────────────────────────────────────────┘  |
+---------------------------------------------------------------+
```

## 3.2 Crate 归属与依赖方向

```
正常依赖方向（修复后）:
  luft-cli  ─────┐
  luft-mcp  ─────┼──> luft-service ──> luft ──> luft-core
  loom      ─────┘                  │       ├──> luft-runtime
                                    │       ├──> luft-storage
                                    │       └──> luft-planner
                                    └──> luft-core
```

| Crate | 层 | 职责 |
|-------|----|------|
| `luft-core` | Engine | 契约类型、journal、scheduler、**query/params/phases/json_to_lua**（从 luft-service 下沉） |
| `luft-runtime` | Engine | Lua 沙箱、workflow 校验 |
| `luft-storage` | Engine | checkpoint、event store |
| `luft-planner` | Engine | NL -> Lua 规划 |
| `luft` | Engine | `Luft` 引擎（编排器）+ **run.rs**（从 luft-service 合并） |
| `luft-service` | Service | `WorkflowService` trait + `WorkflowServiceImpl` + Request/Response/Error 类型 |
| `luft-mcp` | Presentation | rmcp Facade，持有 `Arc<dyn WorkflowService>` |
| `luft-cli` | Presentation | CLI args + 交互 + TUI，持有 `Arc<dyn WorkflowService>` |
| `loom/tool-workflow` | Presentation | Loom Tool trait 适配，持有 `Arc<dyn WorkflowService>` |

## 3.3 接口契约

### Presentation -> Service（通过 `WorkflowService` trait）

依赖倒置修复后，`luft-service` 可以依赖 `luft`，`RunHandle` 类型可用于 trait 签名。`start_workflow` 进入 trait，无需区分 trait / 非 trait 方法。

| 方法 | 入参 | 出参 | 同步/异步 | 说明 |
|------|------|------|----------|------|
| `start_workflow` | `ExecuteWorkflowRequest` | `(ExecuteWorkflowResponse, RunHandle)` | async | 统一入口，MCP 丢弃 handle / CLI + Loom 保留 |
| `list_files` | （无） | `Vec<WorkflowFile>` | sync | |
| `list_runs` | `ListRunsRequest` | `ListRunsResponse` | sync | |
| `get_run_status` | `GetRunStatusRequest` | `RunStatusResponse` | sync | |
| `get_run_events` | `GetRunEventsRequest` | `RunEventsResponse` | sync | |
| `cancel_run` | `CancelRunRequest` | `CancelRunResponse` | sync | |

三个调用者持有 `Arc<dyn WorkflowService>`（trait object），通过 trait dispatch 调用。需要直接访问引擎时（CLI 事件订阅、findings），通过 `WorkflowServiceImpl::luft()` 访问器（downcast 或具体类型持有）。

### Service -> Engine（通过 `Luft` pub 方法）

| 方法 | 入参 | 出参 |
|------|------|------|
| `start_script` | `&str` | `RunHandle` |
| `start_resume` | `run_dir: &str` | `RunHandle` |
| `status` | `run_dir: &str` | `Option<StatusOutput>` |
| `list` | （无） | `Vec<StatusOutput>` |
| `events` | `run_dir: &str` | `Vec<AgentEvent>` |
| `report` | `run_dir: &str` | `ReportStatus` |
| `cancel` | `run_dir: &str` | `()` |
| `with_concurrency` | `n: usize` | `Luft` |

## 3.4 调用序列

### MCP execute_workflow（fire-and-forget，丢弃 handle）

```
Client ──JSON-RPC──> LuftMcpServer #[tool]
  └─ Parameters<ExecuteWorkflowRequest> (rmcp 自动反序列化)
     └─ self.service.start_workflow(req)
        ├─ req.validate()
        ├─ resolve_script_source + inject_args_globals
        ├─ validate_workflow(&script)
        ├─ Luft::start_script(&script) -> RunHandle
        ├─ tokio::spawn(write_instance_json)
        └─ return (Response, _handle)     // handle 丢弃
     └─ serde_json::to_string(&resp)
  <──JSON string───
```

### CLI run（阻塞 + 事件流）

```
clap args ──> RunCommand
  └─ service.start_workflow(req) -> (resp, handle)
     ├─ Presentation: 交互确认 script
     ├─ Presentation: auto-fix retry (如果校验失败)
     ├─ handle.subscribe() -> event stream
     ├─ Presentation: TUI 渲染事件流
     ├─ handle.join().await -> RunOutcome
     └─ Presentation: artifact 输出
```

### Loom workflow_start（fire-and-forget + 保留 handle）

```
Agent LLM ──tool_call──> WorkflowStartTool::call(args)
  └─ runtime.service.start_workflow(req) -> (resp, handle)
     ├─ runs.insert(resp.run_id, handle)  // 存入 runs 表
     └─ return resp
  <──ToolCallContent───
```

## 3.5 三层职责

| 层 | 职责 | 不做的事 |
|----|------|---------|
| **Presentation** | Transport 协议（stdio/CLI args/Tool trait）、用户交互、输出格式化 | 业务逻辑、参数校验 |
| **Service** | 参数校验、业务逻辑（workflow 执行、状态推导、事件过滤、分页） | Transport、UI 渲染 |
| **Engine** | 编排器：Lua 运行时、backend 调度、checkpoint、journal | 参数校验、输出格式化 |
