# 程序日志（tracing）— 实现设计

> **状态**: ✅ P1 全部已实现（2026-06-11）— P1a subscriber + P1b 错误埋点 + P1c `agent`/`backend` span + info 生命周期 + P1d ACP 握手/stop_reason/permission(debug) + session/update(trace)；失败可复盘已手验通过，全 215 测试通过。
> **交叉参考**: [`event-logging.md`](./event-logging.md) — 事件日志（互补的另一平面，本文是其 §4 的展开）
> **相关代码**: [`src/core/scheduler/mod.rs`](../../src/core/scheduler/mod.rs)、[`src/adapters/acp_adapter.rs`](../../src/adapters/acp_adapter.rs)、[`src/service/run.rs`](../../src/service/run.rs)

---

## 0. 背景：当前程序日志是黑盒

全仓库 `tracing::*` **只有 5 处、全在 ws 层**，且**无 subscriber**（输出丢弃）；`scheduler`/`adapters`/`runtime`/`sdk`/`service`/`planner` **零 tracing**。一次 agent 失败时，`acp_adapter` 把 spawn/协议/超时错误 `map_err` 成 `BackendError` 直接 return、从不记录；`scheduler` 的重试/耗尽决策也无记录；最终只有一个**不带错因**的 `AgentDone{status:Error}` 事件。**结果：出了事无法复盘**。

本文给出修复方案。核心不是"装个 subscriber"，而是**埋点**——subscriber 只是让埋点可见。

---

## 1. 设计理念

- **用 spans 表达执行层级**，而非孤立日志行。每个工作单元开一个有生命周期、可嵌套的 span；span 内所有日志自动继承其字段（`run_id`/`agent_id`/`attempt`…）。日志因此天然呈现"谁在干什么、嵌套在谁下、为什么"。
- **`run_id` 作连接键**：程序日志每行带 `run_id`，与事件日志 [`events.jsonl`](./event-logging.md) 同键 → 两平面可对照。
- **不重复**：领域结果走事件；程序日志只补**因果/决策/错因**（事件里没有的：`BackendError` 链、重试轨迹）。

---

## 2. Span 层级（方案核心）

```
run{run_id, task}                                   ← service::run / runtime
 └─ phase{phase_id, label}                           ← sdk::control::phase
     ├─ agent{agent_id, phase_id, model, attempt}    ← scheduler::run_agent
     │   └─ backend{backend="opencode"}              ← acp_adapter::run_acp_session
     │        · spawn / initialize / session.new / prompt / stop_reason
     │        · session/update ×N（trace）
     ├─ parallel{span_id, count}                     ← sdk::parallel
     ├─ converge{span_id, rounds}                     ← runtime::converge
     └─ workflow{span_id, path}                       ← sdk::workflow（嵌套 run）
```

实现：用 `#[tracing::instrument]`（正确处理 async 跨 await，不可用 `.entered()` 跨 await）。已落地 `agent` span（[`scheduler::run_agent`](../../src/core/scheduler/mod.rs)，字段 `run_id`/`agent_id`/`phase_id`/`model`）和 `backend` span（[`acp_adapter::run_acp_session`](../../src/adapters/acp_adapter.rs)，字段 `run_id`/`agent_id`/`backend`）；span 内的日志行因此可精简掉重复字段。

> **跨线程嵌套的现实限制（已实测）**：`AgentBackend::run` 用 `spawn_blocking` + `LocalSet` 把 ACP 会话放到**另一线程**，而 tracing 的 span 上下文是**线程本地**、不随 `spawn_blocking` 传播。因此 `backend` span 实际是**根 span、与 `agent` span 平级**（非嵌套）。代价可接受：**每个 span 各自携带 `run_id`/`agent_id` 字段**，复盘照样成立（靠字段而非树形）。真正的树形嵌套需把 span 显式传到对侧线程 `in_scope`，留作后续。
>
> `span_id`（事件日志的 `parallel_*`/`converge_*`/`workflow_*`）与程序日志 span 通过 `run_id`/`agent_id` 对照。

---

## 3. 级别约定

| 级别 | 用途 | 例 |
|---|---|---|
| `error` | 中止某单元的失败 | spawn 失败、`NonRetryable`、`Exhausted`、schema 校验失败、脚本错误、journal 写失败 |
| `warn` | 可恢复/降级 | 可重试错误→将重试、事件 lag 丢帧、连接中途关闭、broadcast send 失败 |
| `info` | 生命周期里程碑（低量高信号） | run 起止+摘要、phase 起止、agent done+status、选定 backend、子 workflow 进出 |
| `debug` | 决策细节 | 重试第 n 次+退避、quota/permit、cache 命中/跳过、ACP 握手步骤、permission 决策 |
| `trace` | 极冗长 | 每条 session/update、原始 payload、逐 token |

---

## 4. 字段约定（结构化，非字符串拼接）

```rust
tracing::warn!(agent_id = %id, attempt, backoff_ms, error = %e, "retryable backend error, retrying");
tracing::error!(agent_id = %id, attempts, error = %e, "agent exhausted retries");
```

标准字段：`run_id` `phase_id` `agent_id` `span_id` `attempt` `backend` `model` `status` `elapsed_ms` `error`(Display 链)。

---

## 5. 埋点地图（落地清单）

按真实执行路径，逐点列：位置 → span/事件 → 级别。

| # | 位置 | 记什么 | 级别 |
|---|---|---|---|
| 1 | [`main.rs`](../../src/main.rs) | 启动 `logging::init`；顶层 `cmd` span | — |
| 2 | [`service/run.rs`](../../src/service/run.rs) | `run` span 开；journal init/open 失败；forwarder `recv` 错误；RunDone 摘要 | error/debug/info |
| 3 | [`runtime/sandbox.rs`](../../src/runtime/sandbox.rs#L88) | 脚本 load/执行错误（带 `ScriptError`） | error |
| 4 | [`scheduler/mod.rs:110-218`](../../src/core/scheduler/mod.rs#L110-L218) `run_agent` | `agent` span；quota 超限(warn)；permit 等待(debug)；**每次 attempt(debug)**；**可重试失败→重试(warn: attempt,backoff,error)**；**耗尽(error: attempts,error)**；**不可重试(error)**；schema 失败(error)；取消(debug)；最终 status(info) | 见括号 |
| 5 | [`acp_adapter.rs:118-243`](../../src/adapters/acp_adapter.rs#L118-L243) | `backend` span；**spawn 失败(error)**；握手 initialize/new/prompt(debug)；**超时/取消/连接关闭(warn/error)**；stop_reason(info)；session/update(trace)；permission 决策(debug) ← **补回现在丢掉的全部错因** | 见括号 |
| 6 | sdk `phase/parallel/converge/workflow` | 各开对应 span（仅上下文，内部少日志，避免和事件重复） | debug |
| 7 | [`planner.rs`](../../src/planner.rs) | 规划起、LLM 调用、解析成功/失败 | info/warn |
| 8 | [`ws/*`](../../src/ws/) | 现有 5 处规范化：连接 accept/close、subscribe、序列化错误 | info/debug/warn |

> 同时把这些点上现在 `map_err(...)?` / `let _ =` **丢掉的错误**（adapters 5、service 9、runtime 6、scheduler 2…）补成对应 `tracing::*`。这是"失败可复盘"的实质。

---

## 6. 输出与控制（[`src/logging.rs`](../../src/logging.rs)）

```rust
pub fn init(level: Option<&str>, default: &str, file: Option<&Path>) -> anyhow::Result<()> {
    let filter = EnvFilter::try_new(level.unwrap_or(""))      // --log-level
        .or_else(|_| EnvFilter::try_from_default_env())       // RUST_LOG
        .unwrap_or_else(|_| EnvFilter::new(default));         // per-subcommand 缺省
    let stderr = fmt::layer().with_writer(std::io::stderr).with_target(false);
    let file_layer = file.map(/* tracing_appender 非阻塞 */);
    let _ = registry().with(filter).with(stderr).with(file_layer).try_init(); // 幂等
    Ok(())
}
```

- 优先级 `--log-level` > `RUST_LOG` > 缺省（`serve`/`listen`=info、`run`=warn）。
- sink：stderr（默认）+ `serve --log-file`（非阻塞 appender）。
- per-subcommand 缺省由 `main.rs` 算好再传入；分发前调用一次。
- JSON 输出、文件轮转 → 后续。

---

## 7. 与事件平面的关联

- **join 键 = `run_id`**：`grep <run_id>` 同时拉出程序日志（stderr/文件）与 `events.jsonl`。
- **分工**：`AgentDone{status:Error}`（事件）告诉你"agent 失败了"；程序日志 `error{agent_id, error=<BackendError 链>, attempts}` 告诉你"因为 spawn opencode 失败、重试 2 次耗尽"。互补不重复。

---

## 8. 验收标准（可验证）

1. **失败可复盘**：一次 agent 失败，默认级别（`run`=warn）的程序日志必须能还原出 `{agent_id, phase_id, BackendError 因, attempt 轨迹, 最终处置}`。
2. **端到端可追踪**：`--log-level debug` 下 run→phase→agent→backend span 嵌套，每行带 `run_id`。
3. **关键路径零静默**：journal 写、forwarder、backend 这些 `let _ =` 失败点至少有 debug/warn 记录。

---

## 9. 性能与卫生

- span 在级别关闭时近乎零成本；trace 的原始 payload 用 `tracing::enabled!(Level::TRACE)` 守卫，不启用不序列化。
- **脱敏**：`McpEndpoint.auth_token`、prompt 全文不进默认级别日志（prompt 仅 trace 且截断）。
- 文件用 `tracing-appender` 非阻塞，勿阻塞 async runtime。

---

## 10. 分阶段

| 阶段 | 内容 | 交付价值 |
|---|---|---|
| **P1a** | `logging.rs` + subscriber + `main.rs` 接线 + `Cargo.toml` 加 `tracing-subscriber`/`tracing-appender` | 让日志可见（修"全丢"） |
| **P1b** | **错误路径埋点**（acp_adapter 错因 / scheduler 重试·耗尽 / runtime·service 错误） | 满足验收①"失败可复盘" |
| **P1c** | span 层级 + info 生命周期（run/phase/agent/backend） | 验收②端到端叙事 |
| **P1d** | debug/trace 细节（握手、session/update、决策） | 深度排障 |

---

## 11. 测试

- **`logging::init` 幂等**：重复调用不 panic、不重复装载。
- **级别优先级**：`--log-level` 覆盖 `RUST_LOG` 覆盖缺省（解析层单测）。
- **失败可复盘（集成）**：用会失败的 mock backend 跑一个 run，捕获 tracing 输出（`tracing-subscriber` 的测试 writer），断言含 `agent_id` + 错因 + 重试轨迹。
- **脱敏**：断言默认级别输出不含 `auth_token` / prompt 全文。
