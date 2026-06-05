# mcp 模块架构

> **Maestro MCP 数据面服务器（M4）。** 一个 stdio JSON-RPC 的 MCP server，供 agent 把结构化 findings / artifacts / logs / status 主动上报回 Maestro。

源码：[`src/mcp.rs`](../../src/mcp.rs)

---

## 1. 职责与边界

MCP（Model Context Protocol）数据面是 Maestro 的**结构化上报通道**——与"控制面"（[adapters](./adapters.md) 的 prompt→result）相对。它让 agent 不必把 findings 塞进自由文本里再解析，而是调用工具直接上报到一个线程安全的存储。

```
   agent ──MCP tools/call──► Maestro MCP server (stdio JSON-RPC)
              report_finding                    │
              report_artifacts                  ▼
              report_log                    McpStore
              report_status              (findings/artifacts/
              request_next_task           logs/statuses, RwLock)
```

**边界**：mcp 只依赖 `core` 的 `Finding`/`Severity`/`Location` 与 id 类型。它**不依赖** scheduler/runtime/adapters——是一个自洽的协议服务器 + 共享存储。

---

## 2. 内部结构

| 组件 | 职责 |
|------|------|
| `McpStore` | 线程安全共享存储：`findings` / `artifacts` / `logs` / `statuses`，全部 `RwLock` |
| `McpEndpointConfig` | 注入 agent 的端点配置（name/url/run_id/agent_id/auth_token） |
| `get_tool_definitions()` | 5 个工具的 JSON Schema 定义 |
| `run_mcp_server(store)` | stdio 主循环：逐行读 → `handle_request` → 写 JSON 响应 |
| `handle_request` | JSON-RPC 方法分发：`initialize`/`tools/list`/`tools/call`/`ping`/`notifications/initialized` |
| `handle_tool_call` | 5 个工具的具体处理，写入 `McpStore` |
| `McpResponse` / `McpError` | JSON-RPC 2.0 响应/错误封装 |
| 上报数据类型 | `ArtifactReport` / `LogReport` / `StatusReport` |

---

## 3. MCP 工具集（数据面契约）

| 工具 | 入参（必填） | 写入 | 说明 |
|------|------------|------|------|
| `report_finding` | kind, severity, title, detail（+ location/evidence/data 选填） | `findings` | 上报结构化发现——核心工具 |
| `report_artifacts` | artifacts[{key, path?, inline?}] | `artifacts` | 上报生成的产物 |
| `report_log` | level, msg | `logs` | 上报日志 |
| `report_status` | status（+ progress/message 选填） | `statuses` | 上报进度/完成状态 |
| `request_next_task` | — | — | converge 队列取下一任务（当前恒返回"队列为空"） |

`report_finding` 是与 [core](./core.md) 的 `Finding` 类型对齐的结构化通道——severity 字符串映射到 `Severity` 枚举，location 映射到 `Location`。

---

## 4. 协议实现

```
run_mcp_server: stdin 逐行 →
   JSON 解析失败 → -32700 Parse error
   handle_request(method 分发):
       initialize              → 返回 protocolVersion "2024-11-05" + serverInfo{maestro,0.1.0}
       tools/list              → get_tool_definitions()
       tools/call              → handle_tool_call(name, arguments)
       ping                    → {}
       notifications/initialized → {}
       其他                    → -32601 Unknown method
   → 每条响应 JSON 一行写回 stdout + flush
```

`McpStore` 提供 `add_finding`/`get_findings`/`clear_findings`/`add_artifact`/`add_log`/`update_status`/`get_status` 等线程安全方法（`RwLock` 守护）。

---

## 5. 设计决策与权衡

- **schema 即契约**：agent 上报 `Finding` 而非自由文本，使下游（converge 投票、report 聚合）能直接消费结构化数据。
- **stdio JSON-RPC**：与 Claude Code / MCP 生态兼容，agent 侧无需特殊适配。
- **存储与协议分离**：`McpStore` 是纯共享状态，`run_mcp_server` 是协议外壳——便于在不同传输（stdio/WebSocket）下复用存储。
- **手写 JSON-RPC 分发**：轻量、无重型 MCP SDK 依赖，代价是协议覆盖面有限（够 v0.1 用）。

---

## 6. 当前状态与局限（v0.1）⚠️

这是当前 Maestro 里**最"已建未联"**的模块，文档需如实标注：

- **尚未接入 agent 运行路径**。[adapters](./adapters.md) 的 `AcpAdapter` 声明 `mcp_injection=false`，且 runtime 的 `build_task` 把 `mcp_endpoint` 设为 `None`——即**当前没有 agent 真正连到这个 MCP server**。findings 目前仍走 [result_collector](./adapters.md#53-result_collector-组装-agentresult) 的文本回退解析。
- **`agent_id` 未关联**：`handle_tool_call` 里所有上报的 `agent_id` 都填 `uuid::nil()`——还没有把上报归属到具体 agent 的机制。
- **`request_next_task` 是桩**：恒返回"队列为空 / isError"，converge 任务队列尚未接线。
- `run_mcp_server` 提供了 stdio 入口，但把它编织进每个 agent 会话（注入端点 + 启动 server）是 P1 工作。

> 简言之：协议与存储**规范完整、可独立运行与测试**，但"agent → MCP → Maestro"的端到端注入是后续里程碑。把它与现状区分清楚，是阅读本模块时最重要的一点。

---

## 7. 相关文档

- 总览：[../architecture.md](../architecture.md)
- 依赖：[core.md](./core.md)（`Finding`/`Severity`/`Location`）
- 协作（P1 联动）：[adapters.md](./adapters.md)（端点注入 + 控制面）、[runtime.md](./runtime.md)（converge 如何消费 findings）
- 旧版设计稿：[../archive/mcp-server.md](../archive/mcp-server.md)
