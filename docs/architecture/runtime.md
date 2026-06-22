# runtime 模块架构

> **mlua 编排运行时。** 在沙箱化的 Lua 5.4 VM 中执行用户的编排脚本，把 SDK 原语（`agent`/`parallel`/`pipeline`/`converge`/…）桥接到 `core` 的调度器与日志层。

源码：[`src/runtime/`](../../src/runtime/) ｜ 公开 API：[`src/runtime.rs`](../../src/runtime.rs)

---

## 1. 职责与边界

`runtime` 是 Maestro 的**编排执行引擎**。它向上承接一段 Lua 脚本（来自 `--workflow` 文件或 `planner` 生成），向下把脚本里的原语调用翻译成对 `core::Scheduler` 的 `run_agent`/`run_parallel` 调用，并按 `JournalStore` 的 cache key 跳过已完成的 agent。

```
   cli::run ──► Runtime::new(scheduler, run_ctx, args, journal, handle)
                     │  注册 SDK 全局 + 应用沙箱
                     ▼
   spawn_blocking ──► Runtime::execute(script)
                     │  lua.load(script).exec()
                     ▼
        脚本调用 agent()/parallel()/pipeline()/converge()/report()
                     │  block_on(...)
                     ▼
        core::Scheduler ──► AgentBackend ──► AgentResult
```

**边界**：脚本**只负责编排**——分支、循环、聚合中间结果。所有"真实工作"（读文件、grep、编辑、联网）都发生在它 `spawn` 的 agent 的 prompt 里。沙箱强制了这条边界：`io`/`os`/`require`/文件/shell 全部被禁。

---

## 2. 内部结构

| 文件 | 职责 |
|------|------|
| [`sandbox.rs`](../../src/runtime/sandbox.rs) | `Runtime` 结构、沙箱施加、10 个 SDK 原语注册、Lua↔JSON 双向转换 |
| [`pipeline.rs`](../../src/runtime/pipeline.rs) | `PipelineExecutor`——多阶段处理引擎 + pipeline 专用事件 |
| [`converge.rs`](../../src/runtime/converge.rs) | `execute_convergence`——对抗性验证算法 + `converge()` SDK 桥接 |
| [`error.rs`](../../src/runtime/error.rs) | `ExecLimits`、`ScriptError`（含 `mlua::Error` 映射） |

公开的便捷函数 `validate(script)` / `validate_script(script)` 只做**语法校验**（不执行），供 `planner` 在生成脚本后做快速回环验证。

---

## 3. 执行模型：阻塞线程 + block_on（关键约束）

这是理解整个 runtime 的核心。SDK 原语需要调用 `async` 的调度器，但 Lua 是同步的。桥接方式是：构造时捕获一个 `tokio::runtime::Handle`，原语内部用 `handle.block_on(...)` 阻塞等待调度结果。

由此产生两条**硬约束**：

1. **`block_on` 不能在 async worker 线程上调用**（会 panic）。
2. **mlua VM 不是 Send-safe 的异步驱动对象**。

因此 `Runtime::execute` **必须从阻塞上下文调用**——见 `cli::execute_runtime` 用 `tokio::task::spawn_blocking` 包裹。Handle 在构造时（async 上下文中）捕获，留给阻塞线程使用。

```
async 上下文 (cli::run)                       阻塞线程 (spawn_blocking)
  Handle::current() ─────捕获───────────────►  rt.execute(script)
                                                  └─ agent() → handle.block_on(sched.run_agent(..))
```

---

## 4. SDK 原语（注册为 Lua 全局）

`register_sdk()` 把以下函数装进 Lua 全局作用域。所有原语共享一个 `phase_counter: Arc<AtomicU32>`——`phase()` 自增它，`agent()`/`parallel()` 读它，使 cache key 和事件携带有意义的 phase id。

| 原语 | 签名 | 语义 |
|------|------|------|
| `agent(opts)` | `{prompt, model?, schema?, backend?, timeout_ms?}` → `{status, ok, output, tokens, findings}` | 跑单个 agent |
| `parallel(items, mapFn)` | `mapFn(item)→opts` → `array<result>` | **栅栏** fan-out，结果保序 |
| `pipeline(params)` | `{items, stages, max_inflight?}` → `{items, ok, failed, …}` | 多阶段处理（见 §6） |
| `converge(items, opts)` | → `{surviving, rounds, converged, findings}` | 对抗性验证（见 §7） |
| `workflow(path, args?)` | → result | 嵌套子工作流（递归 `Runtime::new`） |
| `phase(name, planned?)` | → phase_id | 进度分组，emit `PhaseStarted` |
| `log(msg, level?)` | | emit `Log` 事件 |
| `budget(time_ms?, rounds?)` | | 写 `__budget` 全局（当前仅记录） |
| `report(value)` | | **设置最终输出**（写入 `report_sink`） |
| `json.encode/decode` | | (反)序列化助手 |

`Runtime` 结构本身只保留 `lua` 与 `report_sink`——其余依赖（scheduler、run_ctx、journal、handle）都被 SDK 闭包在构造时捕获。

### `agent()` 与 resume

```
agent(opts) → build_task(opts, phase_id) 产出 (AgentTask, AgentCacheKey, backend?)
   ├─ journal.get_cached(key) 命中? → 直接返回缓存结果(emit "resume: skip" 日志)
   └─ 未命中 → handle.block_on(run_agent) → journal.record_result(key,...) → 返回
```

`parallel()` 同理：对每个 item 调 `mapFn` 得到 opts、建 task、cache 命中的填入结果槽、未命中的批量 `run_parallel`，最后**按输入顺序**组装结果数组。单个 item 失败不抛出，而是变成 `{status="error"}` 槽。

### Lua ↔ JSON 转换

`value_to_json` / `lua_value_from_json` 处理双向转换：table 通过 `raw_len` 区分数组型（`>0` → JSON array）与映射型（→ JSON object）；function/thread/userdata → null。`JsonArg` 是一个 `IntoLua` 包装，让 `'static` 闭包（如 pipeline stage handler）能针对**目标 VM**惰性转换参数。

---

## 5. 沙箱

`apply_sandbox()` 把以下全局置 `nil`，阻断 I/O、OS 访问与动态加载：

```
io  ·  os  ·  debug  ·  package  ·  require  ·  loadfile  ·  dofile  ·  loadstring
```

> ⚠️ **现状**：`ExecLimits`（指令数 1M、墙钟 5min、内存 128MB）已**定义但尚未强制**——`Runtime::new` 的 `_limits` 参数当前未接线到 mlua 的 hook/内存限制。指令/超时/内存熔断是后续加固项。墙钟超时目前由单个 agent 的 `timeout_ms`（在 scheduler 层）兜底，而非脚本级。

---

## 6. Pipeline：文档语义 vs 实际实现（重要）

模块 doc 把 pipeline 描述为"流式、非栅栏"，但**当前实现实际上是逐阶段栅栏 + handler 内联执行**：

```
实际行为:   所有 item 跑完 Stage 0 → 所有 item 跑完 Stage 1 → ...
            每个 handler 同步内联调用(不 spawn 到 worker 线程)
```

**为什么内联**：stage handler 是同步 Lua 闭包，会回调进**单线程的 mlua VM**——而该 VM 已被调用方（阻塞线程）持有。若把 handler spawn 到 worker 线程，会在 VM 锁上死锁。因此 `max_inflight` 字段当前**不产生真正的跨阶段并发**，`timeout_ms` 也未强制。

`PipelineExecutor::execute` 流程：emit `PipelineStarted` → 逐 stage 遍历（emit `PipelineStageStarted`，对每个 item 调 handler、记录 `StageStatus`/耗时、emit `PipelineItemDone`）→ 汇总 `PipelineStats` → emit `PipelineDone`。pipeline 自身不直接消耗 token（`PipelineItemDone.tokens` 为默认值）——真正的 token 消耗发生在 handler 内部调用的 `agent()` 上。

> 真正的流式（item 独立穿过各阶段、不同 item 处于不同阶段）需要把 stage handler 与 Lua VM 解耦，是 v0.2 的演进方向。当前对**纯函数型 stage 链**语义正确，对"每阶段一个 agent"也可用（只是按阶段串行）。

---

## 7. Converge：对抗性验证算法

`execute_convergence()` 是区分 Maestro 与"简单并行"的关键能力——agent 互相验证后结果才交付用户。

```
for round in 1..=max_rounds:
    ① 生产者并行生成 findings        generate_findings(items, producer_prompt, ...)
    ② 对抗者逐 finding 投票          verify_findings(findings, adversary_prompt, ...)
       每个 adversary 返回 Ok 记一票赞成
    ③ 存活判定: approval_ratio ≥ vote_threshold 的 finding 存活
    ④ 收敛判定:
         无存活             → converged, break
         存活数==生成数 && round>1 → converged, break
         否则 存活 finding 作为下一轮 items
```

| 参数（`ConvergeConfig` 默认） | 值 |
|------|-----|
| `adversarial` | true |
| `vote_threshold` | 0.7 |
| `max_rounds` | 3 |
| `producers_per_item` | 1 |
| `adversaries_per_finding` | 1 |

producer prompt 模板用 `{item}` 占位、adversary 用 `{finding}` 占位。findings 来自 `AgentResult.findings`（对 ACP 后端，由 [adapters](./adapters.md) 的 result_collector 从文本解析）。

> ⚠️ **投票数值的边界**：存活判定是 `(count*100)/adversaries ≥ vote_threshold*100`。当 `adversaries_per_finding=1`（默认）时，唯一那位 adversary 必须返回 `Ok`（即 100%）才能让 finding 存活于 0.7 阈值。要让阈值真正起作用，需要把 adversaries 调到 ≥2。

converge 的 phase_id 固定为 2，agent 直接走 `scheduler.run_parallel`/`run_agent`，**不经过 journal 缓存**（即 converge 内部的 agent 当前不参与 resume 去重）。

---

## 8. 错误模型

`ScriptError` 区分：`Syntax`、`SandboxViolation`、`InstructionLimitExceeded`、`WallClockTimeout`、`MemoryLimitExceeded`、`AgentError`、`SerdeError`、`Internal`。`From<mlua::Error>` 把 mlua 的 `SyntaxError`/`RuntimeError` 启发式映射过来（靠错误信息里的关键字识别 sandbox/limit/timeout），`From<serde_json::Error>` 映射序列化错误。

脚本执行失败不会让进程崩溃——`cli` 把它转成 `RunStatus::Failed` 的 `RunDone` 事件并打印错误。

---

## 9. 设计决策与权衡

- **Lua（mlua）而非 JS**：通过 mlua 在 Rust 中原生集成、更轻量、沙箱更可控；代价是生态不如 Node。
- **同步 block_on 桥接而非全异步 Lua**：换取脚本编写心智简单（线性代码即可），代价是必须跑在阻塞线程、且 VM 单线程限制了 pipeline 的真并发。
- **report_sink 单值而非返回值**：脚本通过 `report(v)` 显式声明输出，比"最后一个表达式即结果"更明确、也便于校验"必须调用 report"。
- **resume 在 SDK 层而非调度层**：`agent()`/`parallel()` 自己查 journal，使 cache key（含 phase）与脚本结构对齐；converge/pipeline 暂未接入。

---

## 10. 当前状态与局限（v0.1）

- `ExecLimits` 未强制（§5）。
- pipeline 是逐阶段栅栏 + 内联 handler，非真流式（§6）。
- converge 内部 agent 不参与 resume 缓存（§7）。
- `budget()` 仅写入 `__budget` 全局，运行时未据此熔断。
- `workflow()` 子工作流与父共享同一 scheduler/journal/run_ctx，没有独立的配额/事件隔离。

---

## 11. 相关文档

- 总览：[../architecture.md](../architecture.md)
- 依赖：core.md（Scheduler、JournalStore、AgentEvent、AgentCacheKey）
- 上游：planner.md（如何生成被本模块执行的脚本）、[cli.md](./cli.md)（如何在阻塞线程驱动 `execute`）
- Lua SDK 用法参考：[../sdk-reference.md](../sdk-reference.md)
- 旧版设计稿：runtime.md（已归档）
