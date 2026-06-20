# cli 模块架构

> **命令行入口 + 运行编排 + 输出模式。** clap 解析命令，编排一次 run 的完整生命周期（脚本解析 → journal → scheduler → runtime → 事件落盘 → 报告），并以 headless 形式输出。

源码：[`src/main.rs`](../../src/main.rs)（二进制入口）+ [`src/cli.rs`](../../src/cli.rs)（运行逻辑）

---

## 1. 职责与边界

`cli` 是把所有模块**装配成一次完整运行**的顶层编排者。它是唯一同时接触 `planner`、`runtime`、`core`（scheduler/journal/state）与 `adapters`（backend 工厂）的模块。

```
   main.rs (clap)                          cli.rs (run 编排)
   ├─ Run    ─► run_workflow ──┐
   │            backend 工厂   │           run(backend, args):
   │            NL→planner     │             解析 script/run_id/resuming
   │            审批提示        └──────────►   JournalStore(init|open)
   │                                          Scheduler + 事件总线
   ├─ List/Status/Logs ─► 直接读 RunStore     spawn 事件→RunStore 转发
   ├─ Workflows/Save                          Runtime::new
    └─                                         └─ Headless ──► run_headless
                                                   execute_runtime(spawn_blocking + RunDone)
```

**边界**：`main.rs` 负责**参数与一次性命令**（含 NL 规划、审批、backend 选择）；`cli.rs` 负责**run 的运行时编排**（journal/scheduler/runtime 装配、事件落盘、输出）。两者以 `cli::RunArgs` 为接口。

---

## 2. 命令一览（main.rs）

| 命令 | 处理 | 状态 |
|------|------|------|
| `run <NL>` / `run -w <file>` | `run_workflow` → `cli::run` | ✅（NL 经 [planner](./planner.md)） |
| `run --resume` | 复用最近可恢复 run 的 `workflow.lua` | ✅ |
| `run --headless` | JSONL 事件流输出 | ✅ |
| `run --approve` | 跳过审批提示 | ✅ |
| `run -b <backend>` | backend 工厂：`mock` / `opencode` | ✅ |
| `list [-l N]` | 列出历史 run + 状态 | ✅ |
| `status <id>` | run 状态 + token + phase + findings | ✅ |
| `logs <id> [-l N]` | 事件流日志 | ✅ |
| `workflows` | 列出 `~/.maestro/workflows/*.lua` | ✅ |
| `save <name> <out>` | 保存工作流（当前为占位实现） | ⚠️ |

`backend` 工厂在 `main.rs` 内联模块里：`mock` → `MockBackend`（10ms 成功），`opencode` → `AcpAdapter::default_opencode()`。

---

## 3. run 的完整生命周期（cli::run）

这是 cli 的核心，串起所有模块：

```
① 解析 (script, run_id, resuming):
     --resume   → 找最近带 checkpoint.json 的 run，拒绝终态，读回 workflow.lua
     --workflow → 读文件，新 run_id
     script(NL) → 用 planner 生成的脚本透传，新 run_id
② JournalStore 始终开启:
     resuming  → journal.open(run_id) 重建 cache 索引，打印"N agents cached"
     fresh     → journal.init_run(run_id, task_label) + 落盘 workflow.lua
③ Scheduler::new(default config, registry{backend}, None)
④ 事件总线: broadcast channel(256) → RunContext{run_id, cancel, events}
     scheduler.init_run_with(run_id, events)
⑤ spawn 后台任务: 订阅事件 → 逐条 store.append_event(落盘 events.jsonl + checkpoint)
     ★ 复用 journal 的同一个 RunStore 实例，避免 split-brain checkpoint
⑥ Runtime::new(scheduler, run_ctx, extra_args, ExecLimits, journal, Handle::current())
⑦ 按 mode 分派: run_headless
     → execute_runtime: spawn_blocking(rt.execute(script)) → emit RunDone(status, report)
```

**关键编织点**：journal 与事件落盘共用**同一个 `RunStore`**（`journal.store()`），保证 checkpoint 不分裂；handle 在 async 上下文捕获，供 runtime 的阻塞执行线程使用（见 [runtime.md §3](./runtime.md#3-执行模型阻塞线程--block_on关键约束)）。

---

## 4. 输出模式

| 模式 | 行为 |
|------|------|
| **Headless** | 执行后在 500ms 宽限期内 drain 事件，每条 `AgentEvent` 打印一行 JSONL，最后打印 `{type:"report", run_id, report}` |

`execute_runtime` 是 headless 模式的公共内核：用 `spawn_blocking` 在阻塞线程跑 `rt.execute`（mlua 约束），据结果 emit `RunStatus::Completed`/`Failed` 的 `RunDone` 事件，返回 report 或 `ScriptError`。

---

## 5. 只读命令（直接读 RunStore）

`list`/`status`/`logs` 不启动调度，直接经 `get_run_store` 读 `./.maestro/runs/<id>/`：

- `StatusOutput`：从 `RunCheckpoint` 投影出 run_id/task/status/phase/agent 计数/token/时间戳。
- `list_runs_cmd`：按 `updated_at` 倒序。
- `logs_cmd`：读 `events.jsonl`，每条事件序列化为一行。

运行数据根目录：`./.maestro/runs`（相对当前工作目录）。

---

## 6. 设计决策与权衡

- **journal 始终开启**：每次 fresh run 都落盘脚本与进度，使任意 run 后续可 `--resume`；代价是每个 run 都有磁盘 footprint。
- **单一 RunStore 实例**：事件落盘与 journal 共用，杜绝 split-brain——是 cli 编排里最关键的正确性约束。
- **main 与 cli 分层**：参数/规划/审批（一次性、面向人）与运行编排（面向执行）解耦，`RunArgs` 作接口。
- **NL 脚本透传而非落盘再读**：fresh NL run 把生成脚本直接传给 `cli::run`，避免一次多余的读写往返；文件 run 则让 cli 从磁盘读。
- **审批在执行前**：非 `--approve` 时打印脚本并等待 `y` 确认——对"模型生成的代码即将执行"这一风险点的人工闸门。

---

## 7. 当前状态与局限（v0.1）

- `save` 命令是占位实现（只打印，不真正写入内容）。
- `--resume` 选最近的可恢复 run，不支持按 id 精确恢复（`RunCreationMode::Resume` 已在 core 备好，cli 尚未暴露该入口）。
- headless 的事件 drain 用固定 500ms 宽限期 + 轮询，而非确定性的完成信号。
- 取消（`cancel_cmd`）已在 cli 提供函数，但未接到 `main.rs` 的子命令。

---

## 8. 相关文档

- 总览：[../architecture.md](../architecture.md)
- 装配的模块：[planner.md](./planner.md)（NL→脚本）、[runtime.md](./runtime.md)（执行）、[core.md](./core.md)（scheduler/journal/state）、[adapters.md](./adapters.md)（backend 工厂）
- 旧版设计稿：[../archive/cli.md](../archive/cli.md)
