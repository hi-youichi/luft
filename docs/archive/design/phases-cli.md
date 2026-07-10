# `maestro phases <run_dir>` CLI — 树形布局

> **状态**：方案设计（待评审）
> **最后更新**：2025-08-19
> **目标**：用一个子命令把「phase 全景 + 挂载的 agent 状态」一张图看完，弥补 `status` 只给汇总、`logs` 只给事件流的空白。
> **依赖**：[`meta-extraction.md`](./meta-extraction.md)（方案 B 的 meta 提取 + 持久化） · **事件 ts 扩展**（§2.4）
> **交叉参考**：[`status` 子命令](../../src/commands/status.rs) · [`RunCheckpoint`](../../src/core/state.rs) · [`AgentEvent`](../../src/core/contract/event.rs) · [`service::query`](../../src/service/query.rs)
> **相关代码**：[`src/commands/`](../../src/commands/) · [`src/main.rs`](../../src/main.rs) · [`src/commands/mod.rs`](../../src/commands/mod.rs)

---

## 0. 一句话定位

`maestro status <run_dir>` 给的是**汇总卡片**（`current_phase=2`、`completed_phases: 1`、`agent_results: 5`），`maestro logs <run_dir>` 给的是**事件流时间线**。`maestro phases` 填的是中间这层——**按 phase 拆分，每个 phase 把自己的 agent 挂在下面**。

只暴露一个参数 `run_dir`，不引 flag（详见 §6）。

---

## 1. 输出形状

```
Run 7f3a9c1d-…  status=Running  current_phase=2/3  tokens=1,840  elapsed=42s
  Task: 审计仓库安全问题

Phase 1/3  intake            ok=2 failed=0  [completed]   3.2s
  ┊ 扫描代码库
  ├─ a1b2c3d4  completed  120 tok  findings=0
  └─ e5f6a7b8  completed   80 tok  findings=1
Phase 2/3  analyze           ok=1 failed=1  [running]    12.4s
  ┊ 分析每个模块的使用
  ├─ 9c0d1e2f  completed  640 tok  findings=2
  └─ 3a4b5c6d  running      — tok
Phase 3/3  review            pending
```

要点：

- **Header** 复用 `status` 的 5 个字段（run_id/status/current_phase/total_tokens/elapsed），加一行 `Task:`。
- **Phase 行**：`phase_id/total`、`label`、`ok/failed`、状态徽标 `[completed] / [running] / [pending] / [failed]`、耗时。`detail` 以 `┊` 前缀显示在 phase 行下方（如有）。
- **Agent 行**：8 位短 ID + 状态 + tokens + findings 数。running 状态显示 `— tok`，completed 走 checkpoint 数值。
- **失败不展开原因**：保持单行紧凑，要看 reason 用 `maestro logs <run_dir>`。
- 颜色：可选（检测 `NO_COLOR` / `TERM=dumb` 后关掉），默认不强制。

---

## 2. 数据来源：meta（主） + checkpoint + events（合并）

phase 结构有两条来源路径，`build_phases_view` 优先走 meta 路径，meta 为空时降级到 events 重建。

### 2.1 主路径：`checkpoint.workflow_meta`（方案 B）

| 字段 | 来源 | 说明 |
|---|---|---|
| `status`、`current_phase`、`total_tokens`、`created_at` | `RunCheckpoint` | 直接读 |
| phase 总数 | `checkpoint.workflow_meta.phases.len()` | 声明式，run 初始化时写入 |
| phase `label` | `checkpoint.workflow_meta.phases[i].label` | 声明式 |
| phase `detail` | `checkpoint.workflow_meta.phases[i].detail` | 声明式 |
| 预计 agent 数 (`planned`) | `checkpoint.workflow_meta.phases[i].agents` | 声明式 |
| `completed_phases` 的 `ok/failed` | `RunCheckpoint.completed_phases` | 直接读 |
| Agent 状态（completed） | `RunCheckpoint.agent_results` | 直接读 |
| Agent 状态（running） | `events.jsonl` 的 `AgentStart`（无配对 Done） | running agent 不在 checkpoint |
| Agent `tokens`、`findings` 数量 | `RunCheckpoint.agent_results` | 直接读 |
| Phase 耗时 / agent 耗时 | `events.jsonl` 事件 ts | `PhaseDone.ts - PhaseStarted.ts`；`AgentDone.elapsed_ms` 已有 |
| 总耗时（`elapsed`） | `RunStarted.ts` / `RunDone.ts` | run 未结束时用 `now - RunStarted.ts` |

> **注意**：`PhaseStarted` / `PhaseDone` / `RunDone` 当前没有 `ts` 字段——需先完成 §2.4 的事件 ts 扩展。

### 2.2 降级路径：events 重建（旧 run / 无 meta 脚本）

`checkpoint.workflow_meta` 为 `None` 时（方案 B 之前的旧 run，或手写脚本无 `meta` 声明）：

| 字段 | 来源 | 说明 |
|---|---|---|
| phase 总数 | `events.jsonl` 的 `PhaseStart` 去重 | 动态，事后才知道 |
| phase `label` | `events.jsonl` 的 `PhaseStart.label` | 运行时事件 |
| phase `detail` | ❌ 无 | events 不携带 |
| 预计 agent 数 | `events.jsonl` 的 `PhaseStart.planned` | 运行时事件 |

其余字段同主路径。

### 2.3 降级策略

`events.jsonl` 缺失或解析失败时，不致命——

- phase label 显示 `phase=<id>`（无 label）。
- 耗时显示 `?s`。
- running agent 不可见（只显示已完成 agent）。
- 顶部 header 仍可用。

checkpoint 缺失才报错（沿用 `status` 的 `"run not found or has no checkpoint"` 文案）。

### 2.4 前置改动：事件 ts 扩展

当前 `AgentEvent` 枚举中，只有 `RunStarted` 有 `ts: DateTime<Utc>`。为支持耗时计算，需给以下事件加 `ts`：

```rust
// src/core/contract/event.rs

PhaseStarted { run_id, phase_id, label, planned, ts: DateTime<Utc> },  // + ts
PhaseDone    { run_id, phase_id, ok, failed, ts: DateTime<Utc> },      // + ts
RunDone      { run_id, status, total_tokens, report, ts: DateTime<Utc> }, // + ts
```

- **向后兼容**：`#[serde(default)]` 让旧 `events.jsonl` 反序列化时 `ts = epoch`（`DateTime::<Utc>::default()`），耗时计算返回 `None` → 显示 `?s`。
- **发送侧**：所有 `events.send(AgentEvent::PhaseStarted { ... })` 调用处加 `ts: Utc::now()`。影响 `src/runtime/sdk/control.rs`（phase）、`src/service/run.rs`（RunDone）、`src/core/journal.rs`（PhaseDone 如有）。
- `AgentDone` 已有 `elapsed_ms: u64`，不需要加 ts。
- `AgentStarted` 没有 ts，但配对的 `AgentDone.elapsed_ms` 已覆盖 agent 耗时。

**改动量**：~15 行（3 个字段 + ~6 个 send 调用）。

---

## 3. Phase 列表的构建

### 3.1 有 meta（主路径）

直接从 `workflow_meta.phases` 构建，按数组顺序映射 `phase_id = index + 1`。

> **约定**：meta.phases 的数组顺序 == `phase()` 调用顺序 == phase_id 递增顺序。这是 planner 生成脚本时的契约（DSL_REFERENCE 强制要求），但 Lua 可以在循环 / 条件分支里动态调 `phase()`——此时 meta 声明数和实际 `phase()` 调用数不一定相等。`validate_meta()` 只做 `tracing::warn!`（软约束），不阻止运行。

```rust
if let Some(ref meta) = checkpoint.workflow_meta {
    phases = meta.phases.iter().enumerate().map(|(i, mp)| {
        PhaseRow {
            phase_id: (i + 1) as u32,
            label: mp.label.clone(),
            detail: Some(mp.detail.clone()),
            planned: if mp.agents > 0 { Some(mp.agents) } else { None },
            // ok/failed/status 从 completed_phases + events 补
            ...
        }
    }).collect();
}
```

### 3.2 无 meta（降级路径）

扫描 `events.jsonl`，收集所有 `PhaseStart` 的 `phase_id`，去重升序。若 `completed_phases` 里没有某个 `phase_id`，就标 `pending`。

如果一个 `PhaseStart` 事件都没收到（极早的 run），`pending` 列表为空，只显示 `completed_phases`。

---

## 4. 数据结构

新增 `src/service/phases.rs`，与 `query.rs` 同级，导出聚合后的 view model：

```rust
pub struct PhasesView {
    pub run: RunHeader,
    pub phases: Vec<PhaseRow>,
}

pub struct RunHeader {
    pub run_id: RunId,
    pub task: String,
    pub status: CheckpointStatus,
    pub current_phase: u32,
    pub total_phases: u32,
    pub total_tokens: u64,
    pub elapsed_secs: Option<f64>,
    pub created_at: u64,
    pub updated_at: u64,
}

pub struct PhaseRow {
    pub phase_id: PhaseId,
    pub label: String,
    pub detail: Option<String>,       // ← 新增（meta 路径有值，events 路径为 None）
    pub status: PhaseStatus,
```

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum PhaseStatus { Pending, Running, Completed, Failed }
    pub planned: Option<usize>,
    pub ok: usize,
    pub failed: usize,
    pub elapsed_secs: Option<f64>,
    pub agents: Vec<AgentRow>,
}

pub struct AgentRow {
    pub agent_id: AgentId,
    pub short_id: String,             // 前 8 位 hex
    pub status: String,               // "completed" | "running" | "failed" | ...
    pub tokens: Option<u64>,          // running 时为 None
    pub findings: usize,
    pub cache_key_hash: Option<String>,
}
```

`pub fn build_phases_view(checkpoint, events) -> Result<PhasesView>` 是纯函数，便于单测。

---

## 5. CLI 集成

### 5.1 `src/commands/mod.rs`

新增：

```rust
pub mod phases;
```

### 5.2 `src/commands/phases.rs`

仿 `status.rs` 的两层结构：

```rust
pub fn phases_cmd(run_dir: String) -> Result<()> {
    phases_cmd_inner(&mut std::io::stdout(), run_dir)
}

pub(crate) fn phases_cmd_inner(w: &mut impl Write, run_dir: String) -> Result<()> {
    let base_dir = runs_base_dir();
    let checkpoint = maestro::service::query::get_checkpoint(&run_dir, &base_dir)?
        .ok_or_else(|| anyhow::anyhow!("run not found or has no checkpoint: {}", run_dir))?;

    // 读取 events.jsonl（与 status.rs 同路径：直接文件读取）
    let events_path = base_dir.join(&run_dir).join("events.jsonl");
    let events = read_events(&events_path).unwrap_or_default();   // 缺失容错

    let view = maestro::service::phases::build_phases_view(&checkpoint, &events)?;
    render_phases(w, &view)?;
    Ok(())
}

fn render_phases(w: &mut impl Write, view: &PhasesView) -> io::Result<()> { /* §1 的格式 */ }
```

### 5.3 `src/main.rs`

在 `Commands` 枚举加：

```rust
/// Show phase tree with agents grouped under each phase.
Phases {
    #[arg(help = "Run directory name to inspect")]
    run_dir: String,
},
```

路由到 `commands::phases::phases_cmd(run_dir)`。

---

## 6. 不引 flag

`status` 也没 flag，保持一致。理由：

- `--json` / `--agent` / `--watch` 都是合理扩展，但**不在这版做**——v0 命令保持单一职责，需要时再加不破坏现有用法。
- 如果以后加，按 `status` 同样的 pattern（`fn status_run_cmd_inner(w, …)` 接受 `&mut impl Write`）就能无侵入扩 `--json`。

---

## 7. 测试

仿 `status.rs` 的 `TestEnv` 套路，测 5 个场景：

| 用例 | 场景 | 断言 |
|---|---|---|
| `run_not_found` | run_dir 不存在 | 报错，文案含 "run not found or has no checkpoint" |
| `empty_checkpoint_no_meta` | meta 为空，events 也没有 PhaseStart | header 出现，phase 区块输出 `no phases started yet` |
| `all_pending_with_meta` | meta 声明 3 phase，一个都没开始 | 显示 3 个 `[pending]` phase（不是 `no phases started yet`） |
| `phases_from_meta` | checkpoint 有 `workflow_meta`（3 phase） | 树形输出含全部 3 个 phase，带 label + detail |
| `phases_fallback_no_meta` | checkpoint 无 `workflow_meta`（旧 run） | 降级到 events 重建，label 来自 `PhaseStart` |
| `events_missing` | 只有 checkpoint（含 meta），删 events.jsonl | phase 显示（meta 驱动），agent 显示，耗时为 `?s` |

`phases_from_meta` 和 `phases_fallback_no_meta` 是两条路径的核心区别——前者验证 meta 驱动的完整结构，后者验证旧 run 兼容。

不走 e2e，单元测试覆盖即可（与 `status` 一致）。

---

## 8. 范围之外（v0 不做）

- **颜色 / TTY 检测**：默认纯文本。
- **JSON 输出**：`--json` 留作后续。
- **Watch 模式**：`--watch` 留作后续。
- **单 agent 深挖**：用 `maestro logs <run_dir> | grep <agent_short_id>` 已能凑合。
- **跨 run 对比**：`phases` 永远是单个 run。
- **depends_on 可视化**：meta 有依赖信息，但树形视图暂不画 DAG。

---

## 9. 实施步骤

> **前置**：完成 [`meta-extraction.md`](./meta-extraction.md) Step 1–6（meta 提取 + 持久化 + 全量测试通过）。

0. **前置**：给 `PhaseStarted` / `PhaseDone` / `RunDone` 加 `ts` 字段（§2.4），所有 send 调用处补 `Utc::now()`，`cargo test` 通过。
1. 新建 `src/service/phases.rs`，写 `build_phases_view`（meta 优先 + events 降级）+ 单元测试。
2. 新建 `src/commands/phases.rs`，写 `phases_cmd_inner` + 渲染 + 单元测试。
3. 在 `src/commands/mod.rs` 加 `pub mod phases;`。
4. 在 `src/main.rs` 的 `Commands` 枚举加 `Phases` 分支并路由。
5. `cargo test`、`cargo build --release` 跑通。
6. 用 `cargo run --bin maestro -- run --backend mock` 起一个 run，再用 `phases` 验证树形输出。
7. 用 `.maestro/runs/` 下已有的旧 run（无 meta）验证降级路径。
