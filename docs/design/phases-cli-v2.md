# `phases` CLI 树形视图 — 完整实施计划

> **状态**：待实施
> **最后更新**：2025-08-19
> **前置**：[`meta-extraction.md`](./meta-extraction.md) Step 1–6 已完成（meta 提取 + 持久化 + 568 测试通过）
> **目标**：把 `phases` 从「平表 + label/status」升级为设计文档 [`phases-cli.md`](./phases-cli.md) §1 描述的完整树形视图（header + phase + agent 子行 + 耗时）

---

## 0. 当前状态与差距

`luft phases` 已有一个简化版本在工作，输出如下：

```
=== Phases for phased-hello_1782291115 ===
ID  LABEL      STATUS       AGENTS  DETAIL
1   prepare    in-progress  1       Set up a greeting
2   agent_run  in-progress  1       Run the hello agent
3   report     pending      1       Report the final output

source: meta
```

设计文档 [`phases-cli.md`](./phases-cli.md) §1 的目标是：

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

### 差距矩阵

| 设计要求 | 当前状态 | 缺口 |
|---|---|---|
| Run header（run_id/task/status/tokens/elapsed） | ❌ 没有 header | **新建** |
| Agent 子行（短ID/状态/tokens/findings） | ❌ 没有 agent 行 | **新建** |
| Phase ok/failed/planned/elapsed | ❌ 只有 status | **扩展** |
| `ts` on PhaseStarted / PhaseDone / RunDone | ❌ 缺失 | **加字段** |
| 树形渲染 `┊ ├─ └─` | ❌ 只有平表 | **重写** |
| `build_phases_view` 纯函数 | ✅ 已有骨架 | **重构** |
| meta 优先 + events 降级 | ✅ 已实现 | **保留** |

---

## 1. 实施步骤总览

| Step | 模块 | 改动 | 依赖 |
|---|---|---|---|
| 0 | `event.rs` + 6 处 send 调用 | 给 PhaseStarted/PhaseDone/RunDone 加 `ts` | 无 |
| 1 | `service/phases.rs` | 重构数据结构（RunHeader + AgentRow） | Step 0 |
| 2 | `service/phases.rs` | 重写 `build_phases_view` 聚合逻辑 | Step 1 |
| 3 | `commands/phases.rs` | 树形渲染器 | Step 2 |
| 4 | `commands/phases.rs` + `main.rs` | CLI 入口适配 | Step 3 |
| 5 | `service/phases.rs` + `commands/phases.rs` | 单元测试 | Step 4 |
| 6 | — | 端到端验证（mock run + 旧 run） | Step 5 |

---

## 2. Step 0：事件 ts 扩展（前置）

### 2.1 问题

当前 `AgentEvent` 枚举中，只有 `RunStarted` 有 `ts: DateTime<Utc>`。`PhaseStarted`、`PhaseDone`、`RunDone` 都没有时间戳，无法计算 phase 耗时和 run 总耗时。

### 2.2 改动

**文件**: `src/core/contract/event.rs`

给 3 个变体加 `ts` 字段：

```rust
PhaseStarted {
    run_id: RunId,
    phase_id: PhaseId,
    label: String,
    planned: usize,
    ts: DateTime<Utc>,        // ← 新增
},
PhaseDone {
    run_id: RunId,
    phase_id: PhaseId,
    ok: usize,
    failed: usize,
    ts: DateTime<Utc>,        // ← 新增
},
RunDone {
    run_id: RunId,
    status: RunStatus,
    total_tokens: TokenUsage,
    report: serde_json::Value,
    ts: DateTime<Utc>,        // ← 新增
},
```

### 2.3 向后兼容

`#[serde(default)]` 让旧 `events.jsonl` 反序列化时 `ts = epoch`（`DateTime::<Utc>::default()` = `1970-01-01T00:00:00Z`），耗时计算返回 `None` → CLI 显示 `?s`。

serde 行为：`#[serde(default)]` 在 `enum` 变体的 struct 字段上需要 `#[serde(default)]` 标注每个字段。由于 `DateTime<Utc>` 实现了 `Default`，直接标注即可。

### 2.4 发送侧改动

所有构造这 3 个事件的调用处补 `ts: Utc::now()`：

| 文件 | 行 | 事件 | 改动 |
|---|---|---|---|
| `src/runtime/sdk/control.rs` | ~31 | `PhaseStarted` | 加 `ts: Utc::now()` |
| `src/core/state.rs` | PhaseDone 派发处 | `PhaseDone` | 加 `ts: Utc::now()` |
| `src/service/run.rs` | ~370, ~385 | `RunDone` (×2) | 加 `ts: Utc::now()` |

`AgentDone` 已有 `elapsed_ms: u64`，不需要 ts。
`AgentStarted` 没有 ts，但配对的 `AgentDone.elapsed_ms` 已覆盖 agent 耗时。

### 2.5 受影响的测试构造

所有测试代码中手工构造 `PhaseStarted { ... }`、`PhaseDone { ... }`、`RunDone { ... }` 的地方需补 `ts`：

| 文件 | 位置 | 数量 |
|---|---|---|
| `src/commands/event_log.rs` | ~163, ~214 | 2 |
| `src/storage/writer.rs` | ~1056, ~1065 | 2 |
| `src/service/phases.rs` | tests | 4 |
| `src/service/run.rs` | tests | ~2 |

**改动量**：event.rs ~15 行 + send 调用 ~10 行 + 测试 ~20 行 ≈ **45 行**

---

## 3. Step 1：重构数据结构

### 3.1 目标

当前 `PhasesView` 是一个平铺的 `Vec<PhaseRow>`。重构为树形结构：RunHeader → PhaseRow[] → AgentRow[]。

**文件**: `src/service/phases.rs`

### 3.2 新数据结构

```rust
/// 顶层视图：run header + phase 树。
#[derive(Debug, Clone, Serialize)]
pub struct PhasesView {
    pub run: RunHeader,
    pub source: PhasesSource,
    pub phases: Vec<PhaseRow>,
}

/// Run 级汇总信息（设计文档 §1 的 header 行）。
#[derive(Debug, Clone, Serialize)]
pub struct RunHeader {
    pub run_id: RunId,
    pub task: String,
    pub status: CheckpointStatus,
    pub current_phase: u32,
    pub total_phases: u32,
    pub total_tokens: u64,
    pub elapsed_secs: Option<f64>,
    pub created_at: u64,
}

/// 单个 phase 行（含挂载的 agent 子行）。
#[derive(Debug, Clone, Serialize)]
pub struct PhaseRow {
    pub phase_id: u32,
    pub label: String,
    pub detail: Option<String>,
    pub status: PhaseStatus,
    pub planned: Option<usize>,
    pub ok: usize,
    pub failed: usize,
    pub elapsed_secs: Option<f64>,
    pub agents: Vec<AgentRow>,
}

/// Agent 子行（挂在 phase 下面）。
#[derive(Debug, Clone, Serialize)]
pub struct AgentRow {
    pub short_id: String,           // agent_id 前 8 位 hex
    pub status: AgentStatus,
    pub tokens: Option<u64>,        // running 时为 None
    pub findings: usize,
}
```

### 3.3 废弃的旧结构

删除 `PhaseStatus` 的 `Skipped` 变体（设计文档没有这个状态），改用设计文档的 `Failed`：

```rust
pub enum PhaseStatus {
    Pending,
    Running,
    Completed,
    Failed,
}
```

### 3.4 迁移影响

`commands/phases.rs` 的 `PhasesArgs` 和 `to_plain()` 方法需要重写，因为 `PhaseRow` 结构变了。

---

## 4. Step 2：重写 `build_phases_view`

### 4.1 签名变更

```rust
// 旧签名
pub fn build_phases_view(
    checkpoint: &RunCheckpoint,
    events_path: Option<&Path>,
) -> PhasesView

// 新签名
pub fn build_phases_view(
    checkpoint: &RunCheckpoint,
    events: &[AgentEvent],
) -> PhasesView
```

改为接收 `&[AgentEvent]` 切片而非 `Option<&Path>`，使函数成为纯函数（无 I/O 副作用），文件读取移到 `commands/phases.rs`。设计文档 §4 明确要求「纯函数，便于单测」。

### 4.2 数据合并策略

#### Phase 列表构建

```
if checkpoint.workflow_meta.is_some():
    phases = meta.phases.iter().enumerate().map(|(i, mp)| PhaseRow {
        phase_id: i + 1,
        label: mp.label,
        detail: Some(mp.detail),
        planned: Some(mp.agents),
        ...
    })
else:
    phases = events 里 PhaseStarted 去重升序
    detail = None, planned = Some(event.planned)
```

#### ok / failed / status

```
for each phase_row:
    if checkpoint.completed_phases 有匹配 phase_id:
        row.ok = summary.ok
        row.failed = summary.failed
        row.status = Completed (或 Failed if failed > 0)
    elif phase_id < checkpoint.current_phase:
        row.status = Completed
    elif phase_id == checkpoint.current_phase:
        row.status = Running
    else:
        row.status = Pending
```

#### Agent 子行

```
// 1. 已完成的 agent（checkpoint 数据，确定性）
for (agent_id, cache) in checkpoint.agent_results:
    if cache.phase_id == row.phase_id:
        row.agents.push(AgentRow {
            short_id: agent_id前8位,
            status: AgentStatus::from(cache.status),
            tokens: Some(cache.tokens),
            findings: cache.findings.len(),
        })

// 2. 正在运行的 agent（events 有 AgentStarted 但无配对 AgentDone）
for event in events:
    match event:
        AgentStarted { phase_id, agent_id, .. } if phase_id == row.phase_id:
            if agent_id 不在 checkpoint.agent_results:
                row.agents.push(AgentRow {
                    short_id: agent_id前8位,
                    status: Running,
                    tokens: None,     // ← 设计文档：running 显示 — tok
                    findings: 0,
                })
```

#### 耗时计算

```
// Phase 耗时
row.elapsed_secs = events 里 PhaseDone.ts - PhaseStarted.ts
    (ts 缺失或 epoch → None → 显示 ?s)

// Run 总耗时
header.elapsed_secs = if events 有 RunDone:
    RunDone.ts - RunStarted.ts
else:
    now() - RunStarted.ts   // run 还在跑
    (RunStarted.ts 缺失 → None → 显示 ?s)
```

### 4.3 降级策略

- `events` 为空 → agent 行只有 checkpoint 里的（completed），耗时 `None`
- `checkpoint.workflow_meta` 为 `None` → events 重建 phase 列表
- 两者都没有 → phase 列表为空，header 仍可显示

---

## 5. Step 3：树形渲染器

### 5.1 渲染规则

**文件**: `src/commands/phases.rs`

```rust
fn render_phases(w: &mut impl Write, view: &PhasesView) -> io::Result<()>
```

#### Header 行

```
Run {short_run_id}  status={Status}  current_phase={n}/{total}  tokens={tok}  elapsed={secs}s
  Task: {task}
```

- `short_run_id`：run_id 前 8 位
- `elapsed` 为 `None` 时显示 `?s`
- `current_phase` = 0 时显示 `0/{total}`

#### Phase 行

```
Phase {id}/{total}  {label:<12}  ok={ok} failed={failed}  [{status}]   {elapsed}s
```

- `Pending` 状态省略 `ok/failed` 和耗时
- label 左对齐 12 宽（与设计文档对齐）

#### Detail 行

```
  ┊ {detail}
```

仅当 `detail` 非空时显示。

#### Agent 行

```
  ├─ {short_id}  {status}  {tokens} tok  findings={n}
  └─ {short_id}  {status}  {tokens} tok  findings={n}
```

- 最后一个 agent 用 `└─`，其余用 `├─`
- running agent：`tokens` 显示 `—`，`findings` 省略
- completed agent：`tokens` 显示数值，`findings` 显示数量

### 5.2 对齐算法

Phase label 宽度取所有 label 的 `max().max(8)`，动态填充。

### 5.3 NO_COLOR 检测

```rust
let use_color = std::env::var("NO_COLOR").is_err()
    && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true);
```

默认纯文本（设计文档 §8 范围之外）。颜色扩展留作后续。

---

## 6. Step 4：CLI 入口适配

### 6.1 `commands/phases.rs`

```rust
pub(crate) fn phases_cmd_inner(
    w: &mut impl Write,
    run_dir: String,
    args: PhasesArgs,
) -> Result<()> {
    let base_dir = runs_base_dir();
    let checkpoint = luft::service::query::get_checkpoint(&run_dir, &base_dir)?
        .ok_or_else(|| anyhow::anyhow!("run not found or has no checkpoint: {}", run_dir))?;

    let events_path = base_dir.join(&run_dir).join("events.jsonl");
    let events = read_events(&events_path);   // 缺失返回 vec![]

    let view = luft::service::phases::build_phases_view(&checkpoint, &events);

    if args.json {
        writeln!(w, "{}", serde_json::to_string_pretty(&view)?)?;
    } else {
        render_phases(w, &view)?;
    }
    Ok(())
}
```

### 6.2 `main.rs`

`Commands::Phases` 变体保留现有 `--json` flag。

---

## 7. Step 5：测试计划

### 7.1 `service/phases.rs` 单元测试

| 用例 | 输入 | 断言 |
|---|---|---|
| `meta_with_agents` | meta 3 phase + checkpoint 2 agent_results + events 1 running | 3 phase 行，phase 1 有 3 agent（2 completed + 1 running），running agent tokens=None |
| `fallback_events_agents` | 无 meta + events PhaseStarted ×2 + AgentStarted ×1 + AgentDone ×1 | events 重建 2 phase，1 agent completed |
| `running_agent_no_done` | AgentStarted 无配对 AgentDone | agent 状态 Running，tokens=None |
| `ts_present_elapsed` | PhaseStarted.ts + PhaseDone.ts 都有 | elapsed_secs = Some(diff) |
| `ts_missing_elapsed_none` | 旧 events 无 ts（epoch） | elapsed_secs = None |
| `empty_phases_pending` | meta 3 phase，无 events，无 completed_phases | 3 phase 全 Pending，无 agent |
| `events_empty_agents_from_checkpoint` | meta + checkpoint.agent_results + events=[] | agent 行来自 checkpoint |
| `failed_phase_status` | completed_phases 有 failed > 0 | status = Failed |

### 7.2 `commands/phases.rs` 单元测试

| 用例 | 断言 |
|---|---|
| `run_not_found` | 报错 "run not found or has no checkpoint" |
| `meta_full_render` | 输出含 header / phase / agent 行 / 树形符号 |
| `json_output` | JSON 含 run / phases / agents 嵌套结构 |
| `events_missing` | checkpoint 有 meta，events 文件不存在 → phase 显示，耗时 `?s` |
| `pending_no_agent_rows` | Pending phase 不显示 agent 行 |

### 7.3 向后兼容测试

| 用例 | 断言 |
|---|---|
| `legacy_checkpoint_no_meta` | meta=None → events 降级 |
| `legacy_events_no_ts` | 旧 events 反序列化成功，ts=epoch，耗时 None |

---

## 8. Step 6：端到端验证

```bash
# 1. mock run 生成真实 events
cargo run -- run --backend mock examples/phased_hello.lua
RUN_DIR=$(ls -t .luft/runs/ | head -1)
cargo run -- phases "$RUN_DIR"
cargo run -- phases "$RUN_DIR" --json

# 2. 旧 run 降级路径（无 meta）
cargo run -- phases hello_1782057171

# 3. 旧 events（无 ts）降级
cargo run -- phases organize-doc_1782145454
```

---

## 9. 改动量估算

| 模块 | 文件 | 新增行 | 修改行 | 删除行 |
|---|---|---|---|---|
| 事件 ts 扩展 | `event.rs` | 3 | — | — |
| send 调用 | `control.rs`, `run.rs`, `state.rs` | — | ~10 | — |
| 数据结构 | `service/phases.rs` | ~50 | ~30 | ~20 |
| 聚合逻辑 | `service/phases.rs` | ~120 | ~40 | ~30 |
| 树形渲染 | `commands/phases.rs` | ~100 | ~40 | ~20 |
| 测试构造 | 多文件 | ~20 | — | — |
| 单元测试 | `phases.rs` (service + command) | ~200 | — | ~80 |
| **合计** | | **~493** | **~120** | **~150** |

---

## 10. 风险与缓解

| 风险 | 缓解 |
|---|---|
| `#[serde(default)]` 在 enum 变体 struct 字段上不生效 | mlua 的 `#[serde(default)]` 需在变体内部标注；先写单测验证旧 JSON 反序列化 |
| running agent 列表不完整（events 有截断） | running agent 是 best-effort；checkpoint 数据是权威来源 |
| 动态 phase（Lua 循环里调 phase()）导致 meta != 实际 phase | meta 声明数和实际调用数的 mismatch 只做 `warn!`，phases 视图以 meta 声明为准 |
| 测试用例手工构造事件过于繁琐 | 封装 `make_events()` helper 函数减少样板 |

---

## 11. 不在范围内

- 颜色 / TTY 检测
- `--watch` 实时刷新
- `--agent <id>` 单 agent 深挖
- 跨 run 对比
- `depends_on` DAG 可视化
