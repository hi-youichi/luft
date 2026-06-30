# service 模块架构

> **Presentation-free 运行编排与查询 API。** 把 `planner`、`runtime`、`core`、`storage` 装配成一次完整的 run 生命周期，并提供只读查询接口。是 CLI 二进制（`commands`）与库逻辑之间的唯一接缝。

源码：[`src/service/run.rs`](../../src/service/run.rs)（run 编排）+ [`src/service/query.rs`](../../src/service/query.rs)（只读查询）

---

## 1. 职责与边界

`service` 是**表示层无关的库层**——所有不涉及 stdout/stderr/TTY 的 run 逻辑都在这里。`commands/` 只负责参数解析、审批提示、输出格式化，核心编排委托给 `service`。

```
   commands/ (presentation)           service/ (library)
   ├─ run.rs ──────────────────►  run.rs (RunSpec → prepare → execute)
   ├─ generate.rs ─────────────►  run.rs::resolve_script (NL → Lua)
   ├─ status.rs ───────────────►  query.rs::get_status
   ├─ list.rs ─────────────────►  query.rs::list_runs
   ├─ logs.rs ─────────────────►  query.rs::get_logs / get_events
   └─ ...                        query.rs::get_report / cancel_run
```

**边界**：`commands` 依赖 `service`，`service` 不依赖 `commands`。`service` 依赖 `core`、`runtime`、`storage`、`planner`，但不知道 clap 或 TTY 的存在。

---

## 2. run 子模块 — run 生命周期

### 2.1 四阶段流水线

一次 run 分为 **resolve → assign → prepare → execute** 四步：

```
① resolve:  RunInput → validate_source → ScriptSource → resolve_script
                NL → planner::plan_workflow    (async, 调用 LLM)
                Workflow → fs::read_to_string
                Script → 透传
             resolve_fresh() → RunSpec { run_id, script, task_label, ... }
             resolve_resume() → 从 checkpoint.json + workflow.lua 恢复

② assign:   assign_dir_name(spec, base_dir)
                derive_slug(workflow/nl) + timestamp → ensure_unique

③ prepare:  prepare(spec, backend, base_dir, run_ctx) → PreparedRun
                JournalStore(init|open)          ← resume 索引
                open_db → EventWriter             ← SQLite 结构化写入
                Scheduler + BackendRegistry
                事件转发任务: broadcast → journal + SQLite
                Runtime::new(scheduler, run_ctx, journal, handle)

④ execute:  execute(run_ctx, runtime, script)
                spawn_blocking(runtime.execute)   ← mlua 约束
                → emit RunDone(Completed|Failed)
```

### 2.2 核心类型

| 类型 | 说明 |
|------|------|
| `RunInput` | 输入三选一：`nl` / `workflow` / `script` |
| `ScriptSource<'a>` | 枚举：`Nl(&str)` / `Workflow(&Path)` / `Script(&str)` |
| `RunSpec` | 完全解析后的 run 规格：run_id、script、task_label、resuming、extra_args |
| `PreparedRun` | runtime + journal，待执行 |
| `ResumeCheck` | resume 状态检查：`CanResume` / `NotFound` / `NotResumable(status)` |

### 2.3 关键函数

| 函数 | 职责 |
|------|------|
| `validate_source(&RunInput)` | 确保恰好一个输入源 |
| `resolve_script(ScriptSource, backend)` | NL→planner / 文件读取 / 透传 |
| `resolve_fresh(source, backend)` | 新 run：规划脚本 + 生成 RunId |
| `resolve_resume(run_dir, base_dir)` | 恢复 run：读 checkpoint + workflow.lua |
| `assign_dir_name(spec, base_dir)` | 分配唯一 run 目录名 |
| `latest_resumable(base_dir)` | 找最近可恢复的 run（`--resume` 无 id 时用） |
| `prepare(spec, backend, base_dir, run_ctx)` | 装配 journal + scheduler + SQLite + runtime |
| `execute(run_ctx, runtime, script)` | 阻塞线程执行 Lua，emit RunDone |

### 2.4 事件双写

`prepare()` 内 spawn 一个事件转发任务，订阅 `broadcast::channel<AgentEvent>`，每条事件**同时写入两个持久化目标**：

```
AgentEvent ──► journal (store.append_event)    → events.jsonl + checkpoint.json
           └─► EventWriter (write_event)        → SQLite tables (runs/agents/turns/...)
```

- **AcpRaw 事件跳过**：原始 ACP 消息是实时观测流，不落盘。
- **SQLite 可选**：DB 打开失败时只 warn，journal + JSONL 保持为 source of truth。
- **共用同一个 `RunStore`**：journal 与事件落盘共用实例，杜绝 split-brain checkpoint。

---

## 3. query 子模块 — 只读查询

### 3.1 查询 API

| 函数 | 返回 | 说明 |
|------|------|------|
| `list_runs(base_dir)` | `Vec<StatusOutput>` | 按 updated_at 倒序 |
| `get_status(run_dir, base_dir)` | `Option<StatusOutput>` | 单个 run 状态 |
| `get_events(run_dir, base_dir)` | `Vec<AgentEvent>` | 原始事件流 |
| `get_logs(run_dir, base_dir, limit)` | `Vec<String>` | JSONL 格式事件 |
| `get_findings(run_dir, base_dir)` | `Vec<Finding>` | 结构化 findings |
| `get_report(run_dir, base_dir)` | `ReportStatus` | 从 events.jsonl 反向查找 RunDone report |
| `cancel_run(run_dir, base_dir)` | `()` | 设置 cancel token |

### 3.2 StatusOutput DTO

`StatusOutput` 是查询层的 DTO（非表示层类型），从 `RunCheckpoint` 投影而来：

```rust
struct StatusOutput {
    run_id, run_dir, task, status,
    current_phase, completed_phases,
    total_agents, completed_agents,
    total_tokens, created_at, updated_at,
}
```

**设计决策**：DTO 放在 `service` 层而非 `commands`，使二进制 `commands` 单向依赖 `service`，而非反过来。

---

## 4. 依赖关系

```
service ──► core      (JournalStore, Scheduler, RunContext, RunCheckpoint)
        ──► runtime   (Runtime, ExecLimits, ScriptError)
        ──► storage   (open_db, EventWriter)
        ──► planner   (plan_workflow — NL 规划)
```

---

## 5. 相关文档

- 总览：[../architecture.md](../architecture.md)
- 装配的模块：[core.md](./core.md)、[runtime.md](./runtime.md)、[adapters.md](./adapters.md)
- 调用方：[cli.md](./cli.md)（commands → service 分层）
- 存储层：[storage.md](./storage.md)
