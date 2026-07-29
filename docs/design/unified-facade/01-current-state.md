# 1. 现状

## 1.1 三个调用者，三套独立集成层

```
luft-cli                    luft-mcp                    loom/tool-workflow
  |                           |                           |
  | luft::service::run::*     | WorkflowServiceImpl       | service.rs (自写)
  | luft::service::query::*   |   (impl WorkflowService)  |   LuftBuilder 直接调
  |   直接调底层函数          |   -> Luft::*               |   start_script / resume
  v                           v                           |   RunHandle 管理
  +------------- Luft 引擎 ---------------------------------+
```

| 调用者 | Service 层 | 校验逻辑 | 额外职责 |
|--------|-----------|---------|---------|
| `luft-cli` | 绕过，直接调 `run::*` / `query::*` | 各 command 自行实现 | 交互提示、auto-fix retry、TUI 渲染、artifact 输出 |
| `luft-mcp` | `WorkflowServiceImpl` -> `WorkflowService` trait | Service 层统一校验 | rmcp 序列化 |
| Loom `tool-workflow` | 自写 `service.rs`，直接调 `LuftBuilder` | 自行实现 | 7 个 Tool trait 实现、`WorkflowRuntime` 状态管理、事件桥接 |

## 1.2 Loom `tool-workflow` 现有架构

**位置**: `C:\Users\heycj\dev\loom\agent\tool\tool-workflow\`

**依赖方式**: Loom 的 workspace `Cargo.toml` 通过 `[patch.crates-io]` 将 `luft` 系列 crate 指向本地路径 `../luft/crates/*`。

**7 个 Tool**:

| Tool | 文件 | 委托给 service.rs | 对应 WorkflowService 方法 |
|------|------|-------------------|-------------------------|
| `workflow_start` | `tool_start.rs` | `start_workflow()` | `start_workflow()` |
| `workflow_cancel` | `tool_cancel.rs` | `cancel_workflow()` | `cancel_run()` |
| `workflow_status` | `tool_status.rs` | `read_status()` | `get_run_status()` |
| `workflow_list` | `tool_list.rs` | `list_instances()` | `list_runs()` |
| `workflow_events` | `tool_events.rs` | `read_events()` | `get_run_events()` |
| `workflow_source` | `tool_source.rs` | `read_source()` | （无对应，读 .lua 源文件） |
| `workflow_files` | `tool_files.rs` | `list_workflow_files()` | `list_files()` |

**`service.rs` 核心逻辑**:
- `LuftBuilder::new()` + `.backend()` + `.base_dir()` + `.concurrency()` + `.build()`
- `luft.start_script()` / `luft.start_resume()` - 启动工作流
- `RunHandle` 管理: `.subscribe()` 事件流、`.cancel()` 取消、`.join()` 等待完成、`.run_dir_name()` 获取 ID
- `WorkflowRuntime` struct 持有 `Luft` 实例 + run 句柄注册表 + cancel tokens
- `event_bridge.rs` - Luft `AgentEvent` -> Loom JSON 事件格式转换

**完全不使用 `WorkflowService` trait** - 自行封装了全部逻辑。

## 1.3 重复逻辑一览

| 逻辑 | luft-cli | luft-mcp | loom/tool-workflow |
|------|---------|---------|-------------------|
| script 解析 | 自写 | `resolve_script_source` | `service.rs` 自写 |
| concurrency 校验 | 自写 | `ExecuteWorkflowRequest::validate()` | `LuftBuilder.concurrency()` |
| args injection | 无 | `params::inject_args_globals` | `params::inject_args_globals` |
| workflow 校验 | `validate_workflow` | `validate_workflow` | 无（直接执行） |
| 状态推导 | `luft-service::phases` | `derive_phases` | 直接读 `RunHandle` |
| 事件过滤 | 自写 | `filter_events_since` | 自写 |
| 分页 | 自写 | `request.rs` 方法 | 自写 |
| instance.json 写入 | 无 | `write_instance_json` | 无 |

## 1.4 问题

1. **三套校验各不相同** - CLI 有交互确认 + auto-fix retry；MCP 有 `validate()`；Loom 直接执行不校验。
2. **rich status 推导只在 MCP** - `derive_phases` / `derive_report_and_error` / `filter_events_since` 全在 `WorkflowServiceImpl` 里，CLI 用 `luft-service::phases` 另一套，Loom 直接读 `RunHandle`。
3. **Loom 已有完整集成层但不复用** - `tool-workflow/service.rs` 是和 `WorkflowServiceImpl` 平行的实现，逻辑重复但接口不同。
4. **依赖倒置（根因）** - `luft`（引擎）反向依赖 `luft-service`（应为上层），因为 `query.rs` / `params.rs` / `phases.rs` / `json_to_lua.rs` / `run.rs` 这些本属底层的模块被放在了 `luft-service` 中。这导致 `WorkflowServiceImpl` 无法归入 `luft-service`（循环依赖），只能放在 `luft-mcp`，Loom 无法复用。
