# Loom `tool-workflow` vs Luft `luft-mcp` - 逐工具源码对比

> **对比范围**: Loom `agent/tool/tool-workflow/src/` 全部 6 个工具 vs Luft `crates/luft-mcp/src/tools.rs`（手写 JSON-RPC）+ `tools_rmcp.rs`（RMCP 宏，占位实现）。

## 开发清单

### 已完成

- [x] **Luft 引擎**：新增 `Luft::checkpoint()` 方法，暴露已有的 `get_checkpoint()`（~15 行，零引擎改动）
- [x] **Luft MCP `tools.rs`**：终端 run 状态查询走 `checkpoint` 直接读取，不再 `luft.events()` 全量加载 + `derive_phases()` 遍历配对；运行中 run 保留 event-based 推导
- [x] **Loom `instance.rs`**：`build_instance_meta()` 改为 checkpoint 优先--agents/phases/stats/report 四字段直接读 checkpoint，event-based 推导逻辑保留为 legacy fallback
- [x] **Loom `journal.rs`**：3 处 `AgentResultCache` 构造补齐 `elapsed_ms` / `name` 字段
- [x] **参数解析共享**：`parse_list_limit` / `parse_status_filter` / `parse_cursor` 三组列表查询参数抽取到 `luft_service::params`，Loom `service.rs` 和 Luft MCP `tools.rs` 共用；两边各自的本地副本 + 测试已删除
- [x] **`tools_rmcp.rs` 清理**：移除死代码 `parse_concurrency()` + 6 个重复常量，改用 `params::` 共享常量
- [x] **`tools.rs` 事件查询修复**：`EventsFilter::from_args` / `parse_events_offset` / `parse_events_limit` 返回值非 `Result`，移除错误的 `match` 包装；事件先序列化为 `Value` 再过滤
- [x] **`tools.rs` borrow-after-move 修复**：`tokio::spawn(async move)` 捕获 `run_dir_name` 后 `json!` 再用--clone 一份给 spawn
- [x] **Loom `service.rs` 死测试清理**：移除引用已删本地函数的 14 个 `parse_events_*` / `event_matches_*` 测试
- [x] **全量验证**：Loom 73 测试 + Luft MCP 125 测试 + Luft Service 130 测试全部通过

### 未完成

- [ ] **Luft MCP run 生命周期**：仍为 fire-and-forget，无 finalize 持久化 / cancel 注册表 / instance.json 写入
- [ ] **文档同步**：§2.2 / §3.5 / §4.2 / §5 仍为实施前状态，待后续一并更新

---

## 概述

Loom 和 Luft MCP 共享同一执行引擎（`luft::Luft`），核心的"解析三选一参数 → 调 `start_script`/`start_resume` → 返回 running"流程高度重复（~140 行）。但两者在此之上各走各路：

| 维度 | Loom | Luft MCP |
|------|------|----------|
| run 生命周期 | 完整：finalize 持久化、cancel 注册表、事件流桥接、instance.json | 无，fire-and-forget |
| 状态查询 | **离线**：读磁盘 `instance.json`，可跨进程 | **在线**：调引擎 API，仅当前进程 |
| 事件查询 | 流式读 `events.jsonl`，内存效率高 | 全量加载到内存 |
| args 注入 | `_G._args` 注入 | 不处理 |
| 预检验证 | 无 | `validate_workflow()` 语法+结构校验 |
| Luft 实例 | 每次调用新建 | 共享实例 + `with_concurrency()` |
| 深度限制 | max 3 层嵌套 | 无 |

### 关键发现与已实施改动

文档初版假设 Luft 引擎需要新增 checkpoint 字段才能消除重复。实施时发现 **`checkpoint.json` 已经包含全部引擎级事实**——`agent_results`、`completed_spans`、`event_stats`、`report` 均由 `update_from_event()` 增量维护并持久化。问题不是引擎缺数据，而是两个消费方都在忽略 checkpoint、重新从 `events.jsonl` 推导同一批数据。

基于此发现，做了以下改动（零引擎改动）：

1. **Luft**：新增 `Luft::checkpoint()` 方法，暴露已有的 `get_checkpoint()`（~15 行）
2. **Luft MCP**：终端 run 状态查询走 `checkpoint` 直接读取，不再 `luft.events()` 全量加载 + `derive_phases()` 遍历配对；运行中 run 保留 event-based 推导（需观察 in-flight agent）
3. **Loom**：`build_instance_meta()` 改为 checkpoint 优先——agents/phases/stats/report 四个字段直接读 checkpoint，event-based 函数保留为 legacy fallback（~190 行推导逻辑不再走终端路径）
4. **修复**：`journal.rs` 3 处 `AgentResultCache` 构造补齐 `elapsed_ms`/`name` 字段

两个 workspace 全量编译通过，Loom 114 测试 + Luft MCP 134 测试全部通过。

---

## 1. 架构总览

```
┌─────────────────────────────────────────────────────────────────┐
│                        Loom (tool-workflow)                     │
│                                                                 │
│  tool_start.rs  tool_status.rs  tool_events.rs  tool_list.rs   │
│  tool_cancel.rs tool_source.rs          (薄 Tool trait 包装)    │
│         │                                                       │
│         ▼                                                       │
│  service.rs  ←── 全部业务逻辑集中于此                           │
│   ├─ start_workflow()     (参数解析 + 引擎调用 + 后台 finalize) │
│   ├─ read_status()        (instance.json 读取 + 脱敏)          │
│   ├─ read_events()        (events.jsonl 流式读取 + 过滤分页)    │
│   ├─ list_instances()     (扫描 .loom/instances + .luft/runs)  │
│   ├─ cancel_workflow()    (CancellationToken 注册表查找)       │
│   └─ read_source()        (workflow.lua 读取 + 预览截断)       │
│         │                                                       │
│         ├─ runtime.rs     (WorkflowRuntime: 路径布局,           │
│         │                   active_runs 注册表, finalize,       │
│         │                   checkpoint 状态推断)               │
│         ├─ backend.rs      (LoomAgentBackend: Agent->Luft 桥接) │
│         ├─ event_bridge.rs (Luft<->Loom 事件格式转换)          │
│         ├─ instance.rs     (instance.json 元数据构建/写入)     │
│         ├─ workflow_resolver.rs (.loom/workflows/ 名字解析)    │
│         └─ json_to_lua.rs  (JSON->Lua 表达式注入 _G._args)      │
│                    │                                            │
│                    ▼                                            │
│              luft::Luft  (执行引擎, crates.io 依赖)             │
│               ├─ start_script() / start_resume()               │
│               ├─ start_workflow()                               │
│               └─ spawn_run() -> tokio::spawn(execute())        │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                     Luft (luft-mcp)                             │
│                                                                 │
│  tools.rs  (手写 JSON-RPC dispatch, 实际实现)                   │
│   ├─ execute_workflow()  (参数解析 + 引擎调用, 直接返回)        │
│   ├─ get_run_status_tool()  (luft.status() + 事件扫描推导)     │
│   ├─ get_run_events_tool()  (luft.events() + 内存过滤分页)     │
│   ├─ list_runs_tool()       (luft.list() + 内存分页)           │
│   ├─ cancel_run_tool()      (luft.status() 检查 + luft.cancel())│
│   └─ list_files_tool()      (目录扫描)                         │
│                                                                 │
│  tools_rmcp.rs  (RMCP #[tool] 宏, 占位实现, 未接入引擎)         │
│                                                                 │
│                    │                                            │
│                    ▼                                            │
│              luft::Luft  (同一个执行引擎, 本 crate 直接调用)     │
│               ├─ start_script() / start_resume()               │
│               ├─ with_concurrency()  (clone + 换 concurrency)  │
│               ├─ status() / events() / list() / cancel()      │
│               └─ report()                                       │
└─────────────────────────────────────────────────────────────────┘
```

### 关键架构差异

| 维度 | Loom | Luft MCP |
|------|------|----------|
| 引擎访问方式 | 外部 crate 依赖 (`luft = "0.3"`) | 本仓库内部直接调用 |
| Luft 实例策略 | 每次 `start_workflow` 新建 `LuftBuilder::build()` | 共享单例, per-call `with_concurrency()` clone |
| Agent 后端 | `LoomAgentBackend` (自有实现, 桥接 `agent::Agent`) | 无自定义后端, 使用 Luft 默认 |
| 工具注册 | `Tool` trait 实现 + `ToolSpec` schema | 手写 `match` dispatch (tools.rs) / RMCP 宏 (tools_rmcp.rs) |
| 磁盘布局 | `.loom/instances/<dir>/` (instance.json, checkpoint.json, events.jsonl, workflow.lua) | `.luft/runs/<dir>/` (checkpoint.json, events.jsonl) |
| Run 标识符 | `instance_dir` (目录名) | `run_id` (目录名) - 本质相同, 命名不同 |
---

## 2. 逐工具对比

### 2.1 启动工作流 (workflow_start / execute_workflow)

#### 源码位置

- **Loom**: `tool_start.rs` (Tool trait 壳) -> `service.rs:40-186` (`start_workflow()`)
- **Luft MCP**: `tools.rs:69-161` (`execute_workflow()`)
- **Luft RMCP**: `tools_rmcp.rs:37-73` (`execute_workflow()` - 占位, 未接入引擎)

#### 参数 Schema 对比

| 参数 | Loom (`workflow_start`) | Luft MCP (`execute_workflow`) | 差异 |
|------|------------------------|-------------------------------|------|
| `script` | string, 三选一 | string, 三选一 | 一致 |
| 文件路径 | `workflow` (名字或路径) | `path` (路径) | **命名不同**; Loom 支持 `.loom/workflows/` 名字解析 |
| `resume_from_id` | string, 三选一 | string, 三选一 | 一致 |
| `args` | object, 可选 | object, 可选 | Loom 有 `inject_args_globals()` 注入 `_G._args`; Luft MCP 不处理 args 注入 |
| `concurrency` | integer 1-64, 默认 4 | integer 1-64, 可选 | Loom 默认 4; Luft MCP 不传则用引擎默认 |

#### 执行流程对比

```
                Loom start_workflow()                    Luft MCP execute_workflow()
                ─────────────────────                    ───────────────────────────
 1. 深度检查 (ctx.depth >= 3 -> 拒绝)                     (无)
 2. 解析 resume_from_id / script / workflow              解析 resume_from_id / script / path
 3. 互斥校验                                              互斥校验
 4. (workflow) resolve_workflow() 名字->路径               (path) resolve_script_source() 直接读文件
    -> std::fs::read_to_string()
 5. parse_concurrency (默认 4)                           parse_concurrency (可选, None=引擎默认)
 6. extract_user_args + inject_args_globals              (无 args 注入)
    -> _G._args = {lua_expr}\n{source}
 7. std::fs::create_dir_all(instances_root)              (无, Luft 内部处理)
 8. LoomAgentBackend::new(config_template)               (无, 使用共享 Luft 实例)
 9. LuftBuilder::new()                                   (无)
    .backend(backend)
    .base_dir(&base_dir)
    .concurrency(concurrency)
    .build()
    -> 每次调用创建新 Luft 实例
10. if resume: luft.start_resume(id)                     if resume: luft.start_resume(id)
    else:       luft.start_script(lua)                   else:       validate_workflow(script)  <- Luft 独有预检
                                                                      luft.start_script(script)
11. runtime.register_run(run_dir_name)                   (无 cancel 注册)
    -> CancellationToken 存入 active_runs HashMap
12. cancel token 编排:                                    (无)
    caller_token <-> runtime_token 镜像
    tokio::spawn(caller.cancelled() -> mine.cancel())
13. tokio::spawn(background_finalize(...))                (无, fire-and-forget 后直接返回)
    -> 监听事件流直到 RunDone
    -> 读取 checkpoint.json + events.jsonl
    -> 构建 InstanceMeta
    -> 写入 instance.json
    -> runtime.unregister_run()
14. 返回 { instance_dir, status, resumed_from? }          返回 { run_id, status, resumed_from? }
```

#### 重复部分（步骤 2-3, 10, 14）

核心的"解析三选一参数 -> 互斥校验 -> 调 `luft.start_resume/start_script` -> 返回 running"是逐行级别的重复。具体对应：

| 逻辑 | Loom 位置 | Luft MCP 位置 |
|------|-----------|---------------|
| 解析 `resume_from_id` | `service.rs:52-56` | `tools.rs:70-73` |
| 解析 `script` / `workflow`/`path` | `service.rs:57-58` | `tools.rs:74-81` |
| 互斥校验 | `service.rs:61-65` | `tools.rs:83-88` |
| `luft.start_resume(id)` | `service.rs:119-122` | `tools.rs:103-110` |
| `luft.start_script(lua)` | `service.rs:124-126` | `tools.rs:146-152` |
| 返回 JSON | `service.rs:175-185` | `tools.rs:112-117, 156-160` |
#### Loom 独有部分

- **深度限制** (`service.rs:45-50`): `ctx.depth >= 3` 时拒绝, 防止子工作流无限嵌套
- **`resolve_workflow()`** (`workflow_resolver.rs:3-40`): 在 4 个候选路径中查找:
  1. `<working_folder>/.loom/workflows/<name>.lua`
  2. `~/.config/loom/workflows/<name>.lua`
  3. `<working_folder>/<name>.lua`
  4. `<name>` 原样
- **`inject_args_globals()`** (`service.rs:328-334`): 将 `args` JSON 转为 Lua 表达式, 前置 `_G._args = {lua_expr}\n` 到脚本开头
- **per-call `Luft` 实例** (`service.rs:107-117`): 每次调用都 `LuftBuilder::new().backend(LoomAgentBackend).build()`, 配合 `LoomAgentBackend` 使用 Loom 自己的 `agent::Agent`
- **`background_finalize()`** (`service.rs:188-303`): 后台异步监听 run 完成后:
  - 读取 `checkpoint.json` (含重试, 最多 10 次, 间隔 200ms)
  - 读取 `events.jsonl` 逐行解析
  - 提取大体积 agent 输出 (> `AGENT_OUTPUT_INLINE_LIMIT`) 写入单独文件
  - 读取 `workflow.lua` 源码
  - 调 `build_instance_meta()` 构建 `InstanceMeta`
  - 调 `write_instance_artifacts()` 写入 `instance.json`
  - 失败时调 `write_minimal_failed_instance()` 写入最小失败记录
- **cancel token 注册** (`service.rs:141-158`): `runtime.register_run()` 将 `CancellationToken` 存入 `active_runs` HashMap, 供 `workflow_cancel` 查找; 如果调用方有自己的 cancel token, 会 spawn 一个镜像 task 把 caller token 的取消信号传播到 runtime token
- **事件流桥接** (`service.rs:133` + `background_finalize:232-234`): 如果调用方提供了 `any_stream_event_sender`, 每个事件都会通过 `luft_event_to_json()` 转发给调用方
- **终端状态检测** (`background_finalize:271-277`): 每 100ms 轮询 `terminal_checkpoint_status()`, 检查 `checkpoint.json` 是否已标记 completed/failed/cancelled

#### Luft MCP 独有部分

- **预检验证** (`tools.rs:127-142`): `validate_workflow(&script)` 做语法 + 结构 + schema 启发式校验, 不通过则拒绝启动 (Loom 没有这一步, 直接交给引擎执行)
- **共享 Luft 实例 + `with_concurrency()`** (`tools.rs:94-101`): MCP server 启动时构造一次 `Luft`, per-call 通过 `luft.with_concurrency(n)` clone 出一个仅 concurrency 不同的实例 (Loom 则每次新建完整 `LuftBuilder`)

---

### 2.2 查询状态 (workflow_status / get_run_status)

#### 源码位置

- **Loom**: `tool_status.rs` -> `service.rs:338-424` (`read_status()`)
- **Luft MCP**: `tools.rs:327-552` (`get_run_status_tool()` + `build_rich_status()` + `derive_phases()` + `derive_report_and_error()`)

#### 实现方式对比

| 维度 | Loom | Luft MCP |
|------|------|----------|
| 数据来源 | 磁盘文件优先: `instance.json` -> `checkpoint.json` -> 运行中状态 | 引擎 API: `luft.status()` + `luft.events()` + `luft.report()` |
| 脱敏 | `sanitize_instance_for_public()` 移除内部字段 | 无脱敏, 直接序列化 |
| 运行中状态 | `running_status()` 构建 InstanceMeta-shaped 占位 | `StatusOutput` + 事件推导 phases/agents |
| Phase/Agent 详情 | `instance.json` 中预计算好的 `agents[]` / `phase_spans[]` | 实时从事件流推导 `derive_phases()` |
| Report | `instance.json` 中的 `report` 字段 (finalize 时写入) | `luft.report()` 实时查询 |
| Error | `instance.json` 中的 status 字段 | 从事件流逆序查找 `Log { level: Error }` |
| 旧格式兼容 | `rebuild_summary()` 从 checkpoint.json 重建 | 无 (引擎 API 统一处理) |

**关键差异**: Loom 的状态查询是**离线的**--直接读 `instance.json` 文件, 不需要 `Luft` 实例。Luft MCP 的状态查询是**在线的**--需要 `Luft` 实例的 `status()`/`events()`/`report()` API。这意味着 Loom 可以查询其他进程创建的 run, 而 Luft MCP 只能查询当前进程创建的 run。

#### 返回结构差异

Loom 返回 `InstanceMeta` (经过 `sanitize_instance_for_public` 脱敏):

```json
{
  "schema_version": 1,
  "instance_id": "...",
  "instance_dir": "...",
  "workflow": { "kind": "file", "name": "...", "path": "..." },
  "status": "completed|failed|cancelled|running",
  "created_at": 0,
  "completed_at": 0,
  "total_tokens": 0,
  "total_elapsed_ms": 0,
  "agent_count": 0,
  "agents": [...],
  "phase_spans": [...],
  "event_stats": { "total": 0, "by_type": {} },
  "report": {}
}
```

Luft MCP 返回 `StatusOutput` + 推导字段:

```json
{
  "run_id": "...",
  "status": "completed|failed|cancelled|running",
  "total_tokens": 0,
  "created_at": 0,
  "updated_at": 0,
  "total_phases": 0,
  "phases": [{ "phase_id": 0, "label": "...", "status": "...", "agents": [...] }],
  "report": null,
  "error": null
}
```
---

### 2.3 查询事件 (workflow_events / get_run_events)

#### 源码位置

- **Loom**: `tool_events.rs` -> `service.rs:697-828` (`read_events()`)
- **Luft MCP**: `tools.rs:559-678` (`get_run_events_tool()`)

#### 实现方式对比

| 维度 | Loom | Luft MCP |
|------|------|----------|
| 数据来源 | `std::fs::File::open(events.jsonl)` 逐行流式读取 | `luft.events(run_id)` 一次性全量加载到内存 |
| 过滤方式 | 流式过滤: 逐行读 -> 过滤 -> 跳过 offset -> 取 limit | 全量加载 -> 序列化 -> 过滤 -> skip -> take |
| `offset` | `parse_events_offset()` | `args.get("offset")` |
| `events_limit` | `parse_events_limit()`, 默认 50, clamp 1-500 | 同, 默认 50, clamp 1-500 |
| `types[]` | `HashSet<&str>` + `event_matches_types()` | `Vec<String>` + 迭代匹配 |
| `agent_id` | `event_matches_agent_id()` | 字符串比较 |
| `since_event_id` | (无) | `filter_events_since()` 子串匹配 (兼容旧 API) |
| 分页游标 | `next_offset` | `next_offset` |
| 内存效率 | **高**: 流式读取, 只在过滤后持有事件 | **低**: 全量加载到内存后再过滤分页 |
| 跨进程 | 是 (读磁盘文件) | 否 (依赖 Luft 内存状态) |

**重复部分**: 参数解析逻辑 (`parse_events_offset`, `parse_events_limit`, `parse_events_types`, `parse_events_agent_id`) 几乎完全相同, 只是 Loom 用 `HashSet` 做类型过滤而 Luft MCP 用 `Vec` 迭代。分页游标 (`next_offset`) 计算逻辑也一致。

**关键差异**: Loom 直接读磁盘 `events.jsonl` 文件, 逐行流式处理, 内存效率高且支持跨进程查询。Luft MCP 依赖 `luft.events()` API 将全部事件加载到内存后再过滤分页。

---

### 2.4 列出历史 (workflow_list / list_runs)

#### 源码位置

- **Loom**: `tool_list.rs` -> `service.rs:481-559` (`list_instances()`)
- **Luft MCP**: `tools.rs:223-286` (`list_runs_tool()`)

#### 实现方式对比

| 维度 | Loom | Luft MCP |
|------|------|----------|
| 数据来源 | `collect_instances_under()` 扫描 `.loom/instances/` + `.luft/runs/` 两个目录 | `luft.list()` 引擎 API |
| 每条记录来源 | `instance.json` 优先 -> `checkpoint.json` 回退 | `RunSpec` 元数据 (引擎内部) |
| 排序 | `created_at` 降序, 再按 `instance_dir` 降序 | `luft.list()` 已按最近修改排序 |
| `limit` | 默认 20, max 100 | 默认 20, max 100 |
| `cursor` | 上一页最后一条的 `instance_dir` | 上一页最后一条的 `run_id` |
| `status_filter` | `completed` / `failed` / `cancelled` | 同 |
| 跨进程 | 是 (扫描磁盘) | 否 (依赖引擎内存状态) |

**重复部分**: 分页逻辑 (cursor 查找 -> skip -> take -> next_cursor 计算) 结构一致, `limit`/`status_filter` 参数校验几乎相同。

**关键差异**: Loom 扫描两个磁盘目录 (`.loom/instances/` 新格式 + `.luft/runs/` 旧格式兼容), 每条记录从 `instance.json` 或 `checkpoint.json` 解析。Luft MCP 依赖 `luft.list()` 引擎 API 返回的 `RunSpec` 列表。

---

### 2.5 取消运行 (workflow_cancel / cancel_run)

#### 源码位置

- **Loom**: `tool_cancel.rs` -> `service.rs:440-477` (`cancel_workflow()`)
- **Luft MCP**: `tools.rs:685-726` (`cancel_run_tool()`)

#### 实现方式对比

| 维度 | Loom | Luft MCP |
|------|------|----------|
| cancel 机制 | `WorkflowRuntime::cancel_run()` 查找 `active_runs` HashMap 中的 `CancellationToken` | `luft.cancel(run_id)` 引擎 API |
| 前置检查 | 无 (直接查注册表, 找不到就返回 `not_found_or_terminal`) | `luft.status(run_id)` 检查是否已终端状态 |
| 返回结构 | `{ instance_dir, result, note }` | `{ run_id, result, note }` |
| 结果值 | `cancelling` / `not_found_or_terminal` | 同 |

**关键差异**: Loom 使用自维护的 `active_runs` 注册表 (在 `start_workflow` 时注册), 查找是纯内存操作, 不依赖 Luft 实例。Luft MCP 先调 `luft.status()` 检查是否已终端, 再调 `luft.cancel()` -- 两步都依赖引擎 API。

---

### 2.6 列出工作流文件 (workflow_files / list_files)

#### 源码位置

- **Loom**: `tool_files.rs` (简单包装, 逻辑极少)
- **Luft MCP**: `tools.rs:205-218` (`list_files_tool()`) + `resources.rs` (`list_examples()`)

#### 实现方式对比

| 维度 | Loom | Luft MCP |
|------|------|----------|
| 搜索目录 | `.loom/workflows/` (runtime.workflows_dir()) | `examples/` + `workflows/` (可配置) |
| 返回字段 | 文件名、路径 | 文件名、路径、URI、描述 |

**差异较小**, 两者都是简单的目录扫描, 搜索目录不同。

---

### 2.7 读取源码 (workflow_source) - Loom 独有

#### 源码位置

- **Loom**: `tool_source.rs` -> `service.rs:832+` (`read_source()`)
- **Luft MCP**: 无对应工具

Loom 独有功能: 读取 instance 目录下 `workflow.lua` 文件的源码, 支持预览截断 (`DEFAULT_SOURCE_PREVIEW_LIMIT = 8192` 字符)。

这是 Loom 自己额外持久化的能力 -- `background_finalize()` 在 run 完成时将执行过的 Lua 源码写入 `workflow.lua`。Luft 引擎本身不落这份文件, 所以 Luft MCP 无法实现这个工具 (除非新增落盘逻辑)。

---

## 3. 辅助模块对比

### 3.1 WorkflowRuntime (Loom 独有)

`runtime.rs` 中的 `WorkflowRuntime` 是 Loom 的核心运行时上下文, 持有:

- `config_template: AgentConfig` -- agent 配置模板
- `active_runs: Arc<Mutex<HashMap<String, Arc<CancellationToken>>>>` -- 活跃 run 的 cancel token 注册表

**Luft MCP 无对应物**。Luft MCP 的 `McpServer` 持有一个共享的 `Luft` 实例, 没有 `active_runs` 注册表 -- cancel 直接走 `luft.cancel()` API。

### 3.2 LoomAgentBackend (Loom 独有)

`backend.rs` 中的 `LoomAgentBackend` 实现 `luft_core::contract::backend::AgentBackend` trait, 桥接 Loom 的 `agent::Agent` 到 Luft 引擎:

- `run()`: 从 `AgentConfig` 构建 `Agent`, 执行 agent 任务, 将 Luft 的 `RunContext.events` 桥接到 Loom 的事件流
- 支持 `thread_id` 恢复 (设置 `config.resume_mode = true`)
- 支持 `output_schema` (注入 `StructuredOutputTool`)
- 支持 `allowlist` (工具白/黑名单过滤)

**Luft MCP 无对应物**。Luft MCP 使用 Luft 引擎的默认后端 (不涉及 Loom 的 `agent::Agent` 体系)。

### 3.3 event_bridge.rs (Loom 独有)

两个转换函数:
- `map_loom_event_to_delta()`: Loom `AgentEvent` -> Luft `ProgressDelta`
- `luft_event_to_json()`: Luft `AgentEvent` -> `serde_json::Value`

**Luft MCP 无对应物**。Luft MCP 直接序列化 `AgentEvent` 不做转换。

### 3.4 workflow_resolver.rs (Loom 独有)

`resolve_workflow(name, working_folder)` 在 4 个候选路径中查找 `.lua` 文件:

1. `<working_folder>/.loom/workflows/<name>.lua`
2. `~/.config/loom/workflows/<name>.lua`
3. `<working_folder>/<name>.lua`
4. `<name>` 原样

**Luft MCP 对应物**: `tools.rs:166-181` 的 `resolve_script_source()`, 但更简单 -- 只检查 `script` 参数和 `path` 参数 (直接读文件), 没有名字解析逻辑。

### 3.5 instance.rs (Loom 独有)

`InstanceMeta` 结构体和 `build_instance_meta()` / `write_instance_artifacts()` 函数, 负责:

- 从 `checkpoint.json` + `events.jsonl` 构建 `InstanceMeta` 摘要
- 写入 `instance.json` (包含 agents, phase_spans, event_stats, report 等)
- 大体积 agent 输出写入单独文件

**Luft MCP 无对应物**。Luft MCP 的状态查询 (`get_run_status_tool`) 实时从引擎 API 推导, 不落盘 `instance.json`。

### 3.6 json_to_lua.rs (Loom 独有)

`json_to_lua(value: &Value) -> String` 将 JSON 值转为 Lua 表达式字符串, 用于 `inject_args_globals()` 将 `args` 注入 `_G._args`。

**Luft MCP 无对应物**。Luft MCP 的 `execute_workflow` 不处理 `args` 注入。

---

## 4. 汇总矩阵

### 4.1 功能覆盖

| 功能 | Loom | Luft MCP (tools.rs) | Luft RMCP (tools_rmcp.rs) |
|------|------|---------------------|---------------------------|
| 启动 (script) | yes | yes | 占位 |
| 启动 (文件) | yes + 名字解析 | yes (仅路径) | 占位 |
| 启动 (resume) | yes | yes | 占位 |
| 预检验证 | no | yes | no |
| args 注入 | yes (`_G._args`) | no | no |
| concurrency | yes (默认 4) | yes (可选) | yes (参数定义) |
| 深度限制 | yes (max 3) | no | no |
| 后台 finalize | yes (instance.json) | no | no |
| 事件流桥接 | yes | no | no |
| cancel 注册表 | yes (active_runs) | yes (luft.cancel API) | 占位 |
| 离线状态查询 | yes (读磁盘) | no (引擎 API) | 占位 |
| 离线事件查询 | yes (读磁盘) | no (引擎 API) | 占位 |
| 离线历史列表 | yes (扫目录) | no (引擎 API) | 占位 |
| 源码读取 | yes | no | no |
| 脱敏 | yes | no | no |

### 4.2 重复代码估算

| 重复模块 | Loom 行数 | Luft MCP 行数 | 重复程度 |
|----------|-----------|---------------|----------|
| 三选一参数解析 + 互斥校验 | ~25 行 | ~20 行 | 逐行重复 |
| concurrency 解析 | ~15 行 | ~18 行 | 逻辑相同, 返回类型不同 |
| luft.start_resume / start_script 调用 | ~10 行 | ~15 行 | 逐行重复 |
| 返回 JSON 构造 | ~12 行 | ~12 行 | 结构相同, 字段名不同 |
| 事件过滤参数解析 | ~40 行 | ~35 行 | 逻辑相同 |
| 事件分页游标计算 | ~10 行 | ~10 行 | 逐行重复 |
| 列表分页逻辑 | ~30 行 | ~25 行 | 结构相同 |
| **合计重复** | **~142 行** | **~135 行** | |

### 4.3 不可共享部分

| Loom 独有模块 | 行数 | 依赖原因 |
|---------------|------|----------|
| `background_finalize()` | ~115 行 | 依赖 `WorkflowRuntime` + `InstanceMeta` |
| `WorkflowRuntime` (runtime.rs) | ~200 行 | 依赖 `AgentConfig` + `active_runs` 注册表 |
| `LoomAgentBackend` (backend.rs) | ~250 行 | 依赖 Loom `agent::Agent` 体系 |
| `instance.rs` | ~800 行 | 依赖 `InstanceMeta` schema + finalize 流程 |
| `event_bridge.rs` | ~35 行 | 依赖 Loom `AgentEvent` 类型 |
| `workflow_resolver.rs` | ~48 行 | 依赖 `.loom/workflows/` 约定 |
| `json_to_lua.rs` | ~100 行 | 依赖 Lua 表达式生成逻辑 |

---

## 5. 消除重复的建议

### 方案 A: 抽取共享 crate (推荐)

创建 `luft-workflow-shared` crate, 抽取以下公共逻辑:

- `WorkflowArgs` 结构体 + 解析/校验逻辑 (三选一 + concurrency + args)
- `resolve_script(path, search_dirs)` 通用文件解析
- `start_run(luft, args)` 封装 `start_script` / `start_resume` 调用
- `EventsFilter` + `paginate_events()` 事件过滤分页
- `ListPagination<T>` 泛型分页逻辑

Loom 和 Luft MCP 各自依赖此 crate, 在此之上添加自己的独有逻辑 (Loom: finalize/cancel注册/event bridge; Luft MCP: 预检验证/with_concurrency)。

### 方案 B: Luft MCP 直接复用 Loom 的 tool-workflow crate

由于 Loom 的 `tool-workflow` 已经依赖 `luft` crate, 理论上 Luft MCP 可以直接引入 `tool-workflow` 作为依赖, 复用其 `Tool` 实现。但这会引入 Loom 的 `agent` 体系依赖, 可能不是 Luft MCP 想要的。

### 方案 C: 不消除 (维持现状)

重复的 ~140 行代码量不大, 且两边的独有逻辑远多于重复部分。如果未来 Loom 和 Luft MCP 的演进方向不同 (Loom 偏向离线/跨进程, Luft MCP 偏向在线/单进程), 维持各自独立实现反而更灵活。