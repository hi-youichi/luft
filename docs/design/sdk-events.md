# SDK 函数事件机制（`sdk_*` 事件）— 实现设计

> **状态**: ✅ 已实现（2026-06-11）— 全 207 lib 测试 + 5 个 e2e（含 2 个新增 SDK 事件测试）通过
> **交叉参考**: acp-raw-events.md — 同源思路（事件走现有总线 + WS 过滤 + headless JSONL）；`websocket-server.md` — WS 订阅/事件流
> **相关代码**: [`src/runtime/sdk/`](../../src/runtime/sdk/)、[`src/runtime/converge.rs`](../../src/runtime/converge.rs)、[`src/core/contract/event.rs`](../../src/core/contract/event.rs)

---

## 0. 目标

让**每个 SDK 原语**在被调用时抛出事件，使整条编排脚本在事件流里是 DSL 粒度可观测的——补齐当前"哑"函数（`budget`/`parallel` 边界/`converge`/`workflow`/`report`）的事件断片，与已有的 scheduler 级（`AgentStarted`/`AgentDone`）、ACP 级（`acp_raw`）事件并存。

与 acp-raw-events.md 同源：事件走现有 broadcast 总线，WS `subscribe` 过滤，headless JSONL 自动带上。区别见 §6（落盘、默认订阅、无开关）。

---

## 1. 现状：SDK 函数的事件断片

| Lua 函数 | 模块 | 当前事件 |
|---|---|---|
| `phase(label, planned?)` | [`control.rs`](../../src/runtime/sdk/control.rs#L21) | ✅ `PhaseStarted` |
| `log(msg, level?)` | [`control.rs`](../../src/runtime/sdk/control.rs#L37) | ✅ `Log` |
| `budget(time, rounds)` | [`control.rs`](../../src/runtime/sdk/control.rs#L53) | ❌ 无（只写 Lua 全局 `__budget`） |
| `agent(opts)` | [`single.rs`](../../src/runtime/sdk/agent/single.rs) | ⚠️ 间接：scheduler 发 `AgentStarted`/`AgentDone` |
| `parallel(items, fn)` | [`parallel.rs`](../../src/runtime/sdk/agent/parallel.rs) | ⚠️ 间接：子 agent 经 scheduler 发；**聚合边界本身无事件** |
| `pipeline(...)` | [`pipeline.rs`](../../src/runtime/pipeline.rs#L230) | ✅ `Pipeline{Started,StageStarted,ItemDone,Done}` |
| `converge(...)` | [`converge.rs`](../../src/runtime/converge.rs) | ❌ 无（边界无事件） |
| `workflow(path, args)` | [`workflow.rs`](../../src/runtime/sdk/workflow.rs) | ❌ 无（子 workflow 进入/退出无事件） |
| `report(value)` | [`report.rs`](../../src/runtime/sdk/report.rs#L17) | ❌ 无 |
| `json.encode/decode` | [`report.rs`](../../src/runtime/sdk/report.rs#L30) | ❌ 无（纯 helper，循环热路径） |

「哑」函数：`budget` / `parallel` 边界 / `converge` / `workflow` / `report`。

---

## 2. 设计决策（已锁定）

| 维度 | 决定 | 说明 |
|---|---|---|
| ① 模型 | **强类型**，每函数独立变体 | 不用通用 `SdkCall`；变体爆炸的代价换取强类型 |
| ② Begin/End | 阻塞函数配对，瞬时函数单发 | 见 §3 |
| ③ 范围 | 只补哑函数，不碰 `agent`，排除 `json` | `agent`/`pipeline` 已有事件；`json` 是热路径 helper |
| ④ payload | **全量**，量大接收端解决 | 不截断 args/result |
| ⑤ 开关 | **无开关，默认开** | 不提供 opt-out |
| A 配对 | `span_id`（`AtomicU64`） | Started/Done 共用，消费端直接配对，无需重建嵌套栈 |
| B 错误路径 | 失败也发 `*Done`（带 `error`） | 避免悬空 Started |
| C 落盘 | 进 `events.jsonl`（`get_logs` 可查） | 与 acp_raw 的"不落盘"相反 |
| D 默认订阅 | **包含** | 不像 acp_raw 被默认排除 |
| E `ParallelDone` | 全量塞每项 output | 与 `AgentDone.output` 重复，风险留待 §8 |

---

## 3. Begin/End 跨度模型

对**阻塞型**调用（`parallel`/`workflow`/`converge`）发 `*Started` + `*Done` 一对；**瞬时**调用（`budget`/`report`）单发。

- `*Done` 带耗时（`elapsed_ms`）+ 结果 payload，并在**失败时也发**（B），`error: Option<String>`。
- 嵌套（`workflow` 套 `parallel` 套 `converge`）下 Started/Done 像括号一样包住内部事件；用 `span_id`（A）配对，消费端无需重建栈。

价值：构造块时长、嵌套调用树重建、实时进度（Started↔Done 之间）、错误归因。

---

## 4. 事件清单（8 个新变体）

加到 [`AgentEvent`](../../src/core/contract/event.rs)（`#[serde(tag = "type", rename_all = "snake_case")]`，故类型名即下方注释）：

**瞬时（单发）**

```rust
BudgetSet     { run_id, time_limit_ms: Option<u64>, max_rounds: Option<u32> }        // budget_set
ReportEmitted { run_id, phase_id, report: serde_json::Value }                         // report_emitted
```

**阻塞（Begin/End，带 `span_id`）**

```rust
ParallelStarted { run_id, phase_id, span_id, count: usize }                           // parallel_started
ParallelDone    { run_id, phase_id, span_id, ok: usize, failed: usize,
                  results: serde_json::Value, elapsed_ms: u64 }                       // parallel_done（E：全量 results）

WorkflowStarted { run_id, span_id, path: String, args: serde_json::Value }           // workflow_started
WorkflowDone    { run_id, span_id, path: String, report: serde_json::Value,
                  elapsed_ms: u64, error: Option<String> }                           // workflow_done

ConvergeStarted { run_id, phase_id, span_id, items: usize, max_rounds: u32 }          // converge_started
ConvergeDone    { run_id, phase_id, span_id, rounds: u32, converged: bool,
                  surviving: usize, result: serde_json::Value,
                  elapsed_ms: u64, error: Option<String> }                           // converge_done
```

`span_id: u64`。`workflow` 跨阶段，无 `phase_id`；其余阻塞函数沿用 `agent()` 的 `phase_counter` 读法。

---

## 5. 实现要点

### 5.1 `span_id`

[`SdkContext`](../../src/runtime/sdk/mod.rs) 增 `span_counter: Arc<AtomicU64>` + 访问器。阻塞函数进入时 `fetch_add(1, Relaxed)` 取号，`Started`/`Done` 共用。

### 5.2 错误路径 guard 模式（B）

保证 `?`/早返回也发 `Done`，且携带成功 payload：

```rust
emit(Started { span_id, .. });
let t0 = std::time::Instant::now();
let outcome: mlua::Result<_> = (|| {
    // 原逻辑，内部照常用 ?
})();
emit(Done {
    span_id,
    elapsed_ms: t0.elapsed().as_millis() as u64,
    error: outcome.as_ref().err().map(|e| e.to_string()),
    // 成功字段从 outcome 的 Ok 分支取
    ..
});
outcome
```

不用 Drop guard（无法携带成功 payload）；panic 不发 Done（panic 本就中止 run，可接受）。

### 5.3 落盘（C）—— 无需改动

forwarder 现在只跳过 `AcpRaw`（[`service/run.rs:221`](../../src/service/run.rs#L221)），sdk 事件自然 fall-through 落盘进 `events.jsonl`。

### 5.4 默认订阅（D）—— 无需改动

`passes_filter(None)` 现在只排除 `acp_raw`（`subscription.rs:59`），sdk 事件默认通过；接收端嫌多自己 `filter`。

### 5.5 `converge` 的特殊性（实现：统一为 `cx`）

[`register_converge_sdk`](../../src/runtime/converge.rs) 原签名 `(lua, scheduler, run_ctx, handle)` 不收 `SdkContext`。落地时**直接统一成 `(lua, cx: &SdkContext)`**（与 `pipeline`/其它一致），一次拿到 `events`/`run_id`/`phase_counter`/`span_counter`/`scheduler`/`run_ctx`/`handle`；调用点 [`sandbox.rs:109`](../../src/runtime/sandbox.rs#L109) 同步为 `register_converge_sdk(lua, cx)`。

> `converge` 在 `ConvergeStarted` 之后到 `ConvergeDone` 之间没有 `?` 早返回（prompt 取值有默认、items 用 `filter_map`），故不需要 §5.2 的 guard 闭包——直接在 `execute_convergence` 的 `Ok`/`Err` 两臂各发一次 `ConvergeDone`。

---

## 6. 与 acp_raw 的差异（一表对照）

| 维度 | `acp_raw` | `sdk_*`（本方案） |
|---|---|---|
| 模型 | 通用 + 判别子 | 强类型逐变体 |
| 落盘 | ❌ 跳过 | ✅ 进 `events.jsonl` |
| 默认订阅 | ❌ 排除（需显式 opt-in） | ✅ 包含 |
| 开关 | `--no-acp-raw` opt-out | 无（默认功能） |
| 频率 | 极高（逐 chunk） | 中低（脚本级） |

---

## 7. 改动文件清单

| 文件 | 改动 |
|---|---|
| [`event.rs`](../../src/core/contract/event.rs) | 加 8 个变体 |
| [`sdk/mod.rs`](../../src/runtime/sdk/mod.rs) | `SdkContext` 加 `span_counter` + 访问器 |
| [`control.rs`](../../src/runtime/sdk/control.rs) | `budget` → `BudgetSet` |
| [`report.rs`](../../src/runtime/sdk/report.rs) | `report` → `ReportEmitted` |
| [`parallel.rs`](../../src/runtime/sdk/agent/parallel.rs) | `ParallelStarted/Done` + span + guard |
| [`workflow.rs`](../../src/runtime/sdk/workflow.rs) | `WorkflowStarted/Done` + span + guard |
| [`converge.rs`](../../src/runtime/converge.rs) | `ConvergeStarted/Done` + span + guard + `span_counter` 参数 |
| [`sandbox.rs`](../../src/runtime/sandbox.rs#L109) | `register_converge_sdk` 调用点传 `span_counter` |
| `protocol.rs` | `event_type_name` 加 8 臂（+ 可选 `sdk_events` capability） |
| `subscription.rs` | `event_run_id` 加 8 臂 |

**必然报错（编译器兜底）**：`event_type_name`、`event_run_id` 两处穷尽匹配。
**无需改动**：`service/run.rs`（落盘自然 fall-through）、`subscription.rs` 的 `passes_filter`（默认通过）、`pipeline.rs`（已有事件，③ 范围外）。

---

## 8. 测试

- **每个 emitter 一个单测**：调用后从 broadcast 收到对应变体、字段正确（用现有 `register_*` + 一个 `SdkContext`/broadcast 夹具）。
- **错误路径**：`parallel`/`workflow`/`converge` 各补一个失败测试 —— 仍收到 `*Done{error: Some(..)}` 且 `span_id` 与 `Started` 一致。
- **全变体测试**：`protocol::event_type_name_all_variants`、`subscription::event_run_id_all_variants` 各加 8 条。

---

## 9. 留待后续

- **E：`ParallelDone.results` 与 `AgentDone.output` 重复**（payload 翻倍）。先按全量做；之后若磁盘/带宽吃紧，再改成只带计数或引用。
- **`sdk_events` capability**：是否在 `default_capabilities` 暴露一个聚合能力标记供客户端发现（vs 列全 8 个类型名）——落地时定。
