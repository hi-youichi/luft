# 跨模块集成与测试策略

> **来源**: 从 `maestro-v0.1-code-design.md` §9–§10 拆分
> **状态**: 设计文档（部分已实现，以代码为准）
> **交叉参考**: 本文档汇总所有模块的集成缝与测试分层

---

## 9. 跨模块数据流与集成

> 本节为收敛产出（Phase 2）：把 6 个模块的设计缝合成端到端视图，并显式记录各模块设计之间需要在实现期统一的几处契约对齐点。

### 9.1 端到端时序（`maestro run "<NL>"` 完整链路）

```
用户 ── maestro run "审计 src/routes/ 鉴权" ──────────────────────────────────────
  │
  ▼ cli::orchestrate_run（§8.4）
  ├─[1] RunId::new_v7 → planner.plan(req)
  │        └─ planner（§7）：LLM 生成 Lua → Runtime::validate（§4.6）→ 失败反馈重试 → 落盘 workflow.lua
  ├─[2] approval：展示 workflow.lua，y 批准（headless/--yes 跳过）
  ├─[3] StateStore::create（§3.3）：写 meta.json(Running) + workflow.lua；spawn EventSink（§3.6）→ events.jsonl
  ├─[4] MaestroMcpServer::start（§5.6）：bind 127.0.0.1:0，得 endpoint
  ├─[5] Scheduler::init_run（§2.3）→ broadcast Receiver；TUI / headless 各 resubscribe
  ├─[6] Runtime::new + execute(script)（§4.2）
  │        Lua 协程逐步调用 SDK：
  │          agent({...}) ──► SchedulerHandle::run_agent（§4.8 → 适配 §2.3 Scheduler）
  │            │   ├─ StateStore::lookup_cached（§3.7）命中？→ 返回缓存（resume 路径）
  │            │   ├─ 配额/semaphore 闸（§2.4）
  │            │   ├─ ServerHandle::agent_mcp_config(agent_id)（§5.5）→ 填 AgentTask.mcp_endpoint
  │            │   ├─ AcpAdapter::run（§6.3）
  │            │   │     spawn opencode acp → initialize → session/new(mcpServers) → session/prompt
  │            │   │       session/update 流 ──► ProgressDelta ──► ctx.events ──► broadcast
  │            │   │       OpenCode ──MCP report_finding──► ResultCollector（§5.2.2）
  │            │   │     collect：findings 优先 / 最终消息回退 → AgentResult（§6.8）
  │            │   └─ StateStore::finish_agent（§3.3）落盘 + 发 AgentDone
  │          parallel(list, fn) ──► 多个 agent 并发（§2.4 run_parallel；semaphore≤16）
  │          converge(items, {...}) ──► reviewers 个 agent 投票收敛（§4.5）
  │          report(value) ──► report_sink（§4.5）
  ├─[7] finalize_run（§8.4）：drain findings/artifacts；写 report.json；StateStore::finish(status)；broadcast RunDone
  └─[8] TUI 收到 RunDone 退出（§8.5.3）/ headless 输出退出码（§8.6）

事件总线（§1.4 AgentEvent）单一数据源，三个消费者并行：① TUI reduce ② headless JSONL ③ EventSink→events.jsonl
```

### 9.2 集成缝与契约对齐点（实现期需统一）

各模块独立设计，下列接缝在实现时必须二选一对齐，建议在冻结 §1 契约的同次评审中一并定下：

| # | 接缝 | 两侧现状 | 建议对齐 |
|---|---|---|---|
| C1 | **Scheduler ↔ runtime 的调度接口** | `core::Scheduler::run_agent` 返回 `Result<(AgentResult, TaskHandle), SchedulerError>`（§2.3）；runtime 期望的 `SchedulerHandle::run_agent` 返回 `Result<AgentResult, BackendError>`（§4.8） | 在 core 提供薄适配器 `impl SchedulerHandle for SchedulerRunBinding`：内部持 `Arc<Scheduler>` + `run_id`，丢弃 `TaskHandle`（取消改走 `RunContext.cancel`），把 `SchedulerError` 映射为 `BackendError`。runtime 只见 `SchedulerHandle` |
| C2 | **findings 收集的持有形态** | adapter 用 `Arc<Mutex<Vec<Finding>>>` 直连（§6.8）；mcp 用 `DashMap<AgentId, Vec<Finding>>` 集中（§5.2.2） | 统一为 mcp 的 `ResultCollector`。adapter 在 `collect` 时调用 `server_handle.findings_for(agent_id)` 拉取，不再各持 Mutex；`AcpSession` 不持 `findings_rx` |
| C3 | **`ScriptError` 变体命名** | runtime 用 `Syntax`/`SandboxViolation`（§4.2）；planner 伪代码用 `SyntaxError`/`ForbiddenGlobal`（§7.6） | 以 runtime（§4.2）为准；planner 的 `format_validation_error` 按 §4.2 变体名匹配 |
| C4 | **`Runtime::validate` 形态** | runtime 定义为关联函数 `Runtime::validate(script)`（§4.6）；planner 调实例方法 `runtime.validate(&script)`（§7.7） | 采用关联函数（§4.6）；planner 不持 `Runtime` 实例，`Planner::new` 去掉 `runtime` 参数（§7.3 已按此） |
| C5 | **`AgentStatus` 序列化大小写** | 契约 `enum AgentStatus { Ok, Error, Cancelled, TimedOut }` 无 `rename`（§1.2）→ Lua 侧看到 `"Ok"`；planner few-shot / audit.lua 统一用 `"Ok"`（§7.5/§8.8） | 保持契约默认 PascalCase；所有脚本与 prompt 用 `"Ok"`。若偏好小写，则在 §1.2 加 `#[serde(rename_all="snake_case")]` 并全局同步 |
| C6 | **`StateStore` 脚本落盘 API** | §3 列了 `create`（创建时写 workflow.lua）；planner 需要独立的 `write_workflow`（§7.7） | 调整 `create` 不写脚本；新增 `StateStore::write_workflow(run_id, script)` 与 `update_meta(...)`，由 planner 在校验通过后调用 |
| C7 | **脚本内序列化辅助** | audit.lua/deep_research.lua 用 `json_encode`（§8.8） | 要么在沙箱白名单注入纯编码的 `json.encode/decode`（无 I/O，安全），要么改用 `put/get_shared_artifact` 传结构化值。推荐前者，简单且与脚本可读性一致 |
| C8 | **`PhaseId` 的产生** | 契约定义 `PhaseId`（§1.1），但 runtime 的 `agent()`/`parallel()`/`converge()` 未明确何时递增 phase | 约定：runtime 每进入一次 `parallel`/`converge` 顶层调用即分配新 `PhaseId`（递增），裸 `agent()` 归入"当前 phase"；由 runtime 维护 `current_phase` 计数器并塞入 `AgentTask.phase_id` |

> C1/C2 是最关键的两处——它们决定 core/runtime/adapters/mcp 能否真正联调通。建议 MS1（核心可跑）前先用 `MockBackend` + `MockSchedulerHandle` 把 C1 打通，MS2（单后端打通）前把 C2 落地。

### 9.3 依赖方向复核

实际类型依赖与计划 §3.1 的单向图一致，无环：

```
cli ──► planner ──► runtime ──► core(contract + scheduler + state)
 │         │                          ▲
 ├──► runtime ─────────────────────────┤
 ├──► adapters ──► core(contract) ─────┤   adapters 实现 AgentBackend
 └──► mcp ──► core(contract) ──────────┘   mcp 复用 Finding/McpEndpoint
```

- `core::contract` 是所有模块的共同底座（§1），无上游依赖。
- `runtime` 经 `SchedulerHandle`（§4.8）解耦 core 具体调度器（C1 适配）。
- `adapters`/`mcp` 只实现/复用 core 契约，互不依赖（findings 经 core 结果池间接交互，C2）。
- `cli` 是唯一组装层，注册后端、构造 Scheduler/Runtime/Planner/MCP server。

---

## 10. 测试与 CI 策略汇总

### 10.1 测试分层

| 层级 | 范围 | 依赖 | CI |
|---|---|---|---|
| **单元** | 各模块纯逻辑：调度并发/配额/重试/取消（§2.9）、沙箱与错误映射（§4.9）、缓存键与状态机（§3.9）、MCP collector/token（§5.8）、权限决策与 update 映射（§6.10）、planner 重试（§7.8）、TUI reduce（§8.9） | 无外部进程；用 `MockBackend`/`MockSchedulerHandle`/mock LLM | **必跑** |
| **集成（进程内）** | MCP server 起停 + reqwest 直打工具（§5.8 T7–T11）；core+runtime+mock backend 全链路（§8.9 e2e） | 仅本进程 tokio | **必跑** |
| **集成（真实 opencode）** | AcpAdapter 经真实 `opencode acp` 跑小 prompt（§6.10）、MCP↔ACP 联调（§5.9） | 本机 `opencode` CLI 1.14.22+ | `#[ignore]`，**本地/夜间** |

### 10.2 关键验收测试映射（计划 §1 的 A1–A10）

| 验收 | 测试 |
|---|---|
| A3 运行时执行 | `runtime` SDK 桥接单测 + e2e mock 全链路 |
| A4 并发调度 | `test_concurrency_limit` / `test_parallel_concurrency_bounded` / `test_quota_*` |
| A5 后端驱动 | `test_acp_full_round_trip`（#[ignore]） |
| A6 数据平面 | MCP `report_finding` 联调（§5.9 I3）+ adapter findings 优先（§6.10） |
| A7 落盘与恢复 | **`test_run_kill_resume_no_rerun`（§3.9 T8）** + `test_resume_skips_completed_agents`（§8.9） |
| A8 沙箱 | `test_sandbox_*` / `test_instruction_limit` / `test_wall_clock_timeout` / `test_memory_limit` |
| A9 观测 | TUI reduce 单测 + `test_headless_exit_codes` |
| A10 示例工作流 | `builtin_workflows_syntax_valid` + e2e 跑 audit/deep-research |

### 10.3 CI 配置要点

- `cargo build` + `cargo nextest run`（沿用仓库现有 nextest 配置风格）。
- 真实 opencode 用例统一加 `#[ignore = "requires opencode binary"]`，CI 默认不跑；夜间 job 用 `cargo nextest run --run-ignored all` 跑。
- 并发/取消类测试用 `tokio::time::pause`/`advance` 控时，避免 sleep 引入 flaky；stress 类（1000 agent 配额、16 并发 report）标 `#[ignore]` 或单列 job，防拖慢主 CI。
- 覆盖率门槛聚焦 `core`/`runtime` 两个关键路径模块（计划 §5.1 标定的瓶颈）。

---

*本代码设计文档以 [v0.1 开发计划](../archive/v0.1-dev-plan.md) 为依据，§1 冻结契约为各模块共同基准。§9.2 的 C1–C8 接缝需在开工前评审一并定下；其余模块内部设计可独立并行实现。*

