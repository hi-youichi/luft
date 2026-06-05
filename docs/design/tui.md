# TUI 实时进度视图设计（P2-1）

> **状态：设计稿（未实现）。** 取代 [`design/cli.md` §8.5](./cli.md) 的旧草图——后者早于 `Pipeline*` 事件，且假设了不存在的 `Pause`/`StopAgent` 调度控制。本文对齐**当前代码**（[`event.rs`](../../src/core/contract/event.rs)、[`cli.rs`](../../src/cli.rs)、[`scheduler/mod.rs`](../../src/core/scheduler/mod.rs)），是 [roadmap P2-1](../roadmap-p1-p2.md) 的落地设计。

把 [`cli.rs` `run_tui`](../../src/cli.rs#L350) 当前的 **println-only 桩**（执行完只打印最终 report）替换为 **ratatui 实时视图**：消费已有的 `AgentEvent` 广播流，渲染 `phase / pipeline → agent → token / 耗时 / 工具` 的实时进度，并支持取消。

---

## 0. 目标与范围

**做：**
- 实时消费 `run_ctx.events`（`broadcast::Receiver<AgentEvent>`），把事件流折叠成可视状态。
- 折叠式 phase 树 + pipeline 阶段视图 + 逐 agent 行（状态 / token / 耗时 / 最近工具 / 活动行）。
- 键盘导航（↑↓ / Enter 展开折叠）与取消（`c` 取消整 run、`x` 取消选中 agent）。
- 终端 raw mode + alternate screen 的安全进入/退出（含 panic 钩子复原）。
- `maestro workflows` 的**只读**列表 + 详情视图（读 checkpoint + `events.jsonl`）。

**不做（v0.2 非目标）：**
- **暂停/恢复**：调度器无 pause 原语（仅 `cancel_run` / `cancel_agent`），不在本期实现。
- **跨进程 live-attach**：附着到**另一个进程**正在跑的 run 需要 IPC/共享事件总线，列为未决（§9）。
- 鼠标交互、主题配置、日志面板滚动检索——后续迭代。

---

## 1. 集成点：`run_tui` 如何改造

### 现状（[`cli.rs:350`](../../src/cli.rs#L350)）

```rust
async fn run_tui(run_ctx, rt, script) -> Result<()> {
    let result = execute_runtime(&run_ctx, rt, script).await?; // 阻塞到执行结束
    sleep(50ms).await;                                          // 等落盘 flush
    println!("=== Report ===\n{pretty_json}");                 // 打印最终 report
}
```

执行是**先跑完再输出**——无法实时展示。

### 改造后（并发模型）

关键：**先订阅事件，再 spawn 执行**，主任务跑 TUI 事件循环，执行在独立 task 上推进，`RunDone` 由 [`execute_runtime`](../../src/cli.rs#L286) 在末尾广播（保持不变），循环见到 `RunDone` 即收尾。

```rust
// 签名增加 scheduler（用于 cancel_agent）。run() 里 scheduler 已存在，clone 传入即可。
async fn run_tui(
    run_ctx: RunContext,
    scheduler: Scheduler,
    rt: Runtime,
    script: String,
) -> Result<()> {
    // ① 在启动执行前订阅，确保不漏掉首批事件（RunStarted/PhaseStarted…）。
    let event_rx = run_ctx.events.subscribe();

    // ② spawn 执行；execute_runtime 内部会在结束时 emit RunDone。
    let exec_ctx = run_ctx.clone();           // RunContext: Clone
    let exec = tokio::spawn(async move { execute_runtime(&exec_ctx, rt, script).await });

    // ③ 进入 TUI 事件循环，直到 RunDone / 流关闭 / 用户退出。
    let outcome = tui::run_app(run_ctx.clone(), scheduler, event_rx).await?;

    // ④ 回收执行结果（report / ScriptError），终端已复原后再打印总结。
    let result = exec.await.map_err(|e| anyhow::anyhow!("exec task panicked: {e}"))??;
    tui::print_summary(&outcome, &result);    // 复原终端后，在普通屏打印最终 report/错误
    Ok(())
}
```

**为什么订阅时机安全**：执行只在 `run_tui` 内 `tokio::spawn` 后才开始，`event_rx` 在 spawn 前创建——广播无早发事件丢失风险。`broadcast` 多订阅者：journal 落盘 forwarder（[`cli.rs:252`](../../src/cli.rs#L252)）与 TUI 各自独立订阅，互不影响。

[`run()` 的 mode 分派](../../src/cli.rs#L276) 改为 `RunMode::Tui => run_tui(run_ctx, scheduler.clone(), rt, script).await?`。`execute_runtime` / `run_headless` **保持不变**。

---

## 2. 模块布局

```
src/tui/
├── mod.rs      ← run_app() 事件循环 · setup/restore_terminal · print_summary · run_app_outcome
├── state.rs    ← AppState/PhaseView/PipelineView/AgentView + reduce() 纯函数（含单测）
├── render.rs   ← ratatui 组件：header / body 树 / footer；纯渲染（state → Frame）
└── input.rs    ← crossterm KeyEvent → TuiAction 映射 + 选择移动/折叠的纯逻辑
```

边界：`reduce` 与 `render` 都是**纯函数**（不持锁、不 IO），便于单测（§11）；`mod.rs` 独占 IO（终端、事件循环）。

---

## 3. 状态模型（`state.rs`）

```rust
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Instant;

pub struct AppState {
    pub run_id: RunId,
    pub task_preview: String,
    pub status: AppStatus,

    // PhaseId = u32（单调递增）→ BTreeMap 即按 phase 顺序；无需 IndexMap 依赖。
    pub phases: BTreeMap<PhaseId, PhaseView>,
    // 一次 run 可有多个 pipeline()，按出现顺序追加；事件无 pipeline_id，故用 Vec + “当前=末项”。
    pub pipelines: Vec<PipelineView>,

    // AgentProgress/AgentDone 仅带 agent_id（无 phase_id）→ O(1) 路由到所属 phase。
    pub agent_phase: HashMap<AgentId, PhaseId>,

    // 选择光标：在“可见行”上的线性索引（render 时与树展开状态一致映射）。
    pub cursor: usize,

    pub total_tokens: TokenUsage,   // 仅 AgentDone(增量) / RunDone(权威覆盖) 更新
    pub concurrency: usize,         // 当前 Running 的 agent 数
    pub quota_used: u32,
    pub quota_limit: u32,           // 来自 SchedulerConfig.quota_per_run（默认 1000）

    pub started_at: Instant,
    pub spinner: usize,             // tick 推进的转轮帧索引
    pub lagged: u64,                // 累计 broadcast Lagged 丢失计数（仅提示）
}

pub enum AppStatus { Running, Done(RunStatus), Cancelling }

pub struct PhaseView {
    pub phase_id: PhaseId,
    pub label: String,
    pub planned: usize,
    pub agents: BTreeMap<AgentId, AgentView>, // uuid v7 时间有序 → 按创建序展示
    pub ok: usize,
    pub failed: usize,
    pub done: bool,
    pub expanded: bool,             // Enter 折叠/展开
}

pub struct PipelineView {
    pub total_stages: usize,
    pub items: usize,
    pub stages: Vec<StageView>,     // 按 stage_index
    pub total_ok: usize,
    pub total_failed: usize,
    pub done: bool,
    pub expanded: bool,
}

pub struct StageView {
    pub index: usize,
    pub label: String,
    pub agents_in_stage: usize,
    pub items_done: usize,          // PipelineItemDone 累加
    pub ok: usize,
    pub failed: usize,
}

pub struct AgentView {
    pub agent_id: AgentId,
    pub status: AgentViewStatus,
    pub prompt_preview: String,     // 截断 ~60 字
    pub model: Option<String>,
    pub tokens: TokenUsage,
    pub elapsed_ms: u64,
    pub started_at: Option<Instant>,
    pub recent_tools: VecDeque<ToolEntry>, // 容量 5，最新在前
    pub last_message: Option<String>,      // 最近一条 ProgressDelta::Message（活动行）
}

pub enum AgentViewStatus { Pending, Running, Done(AgentStatus) }

pub struct ToolEntry { pub name: String, pub summary: String }
```

**设计要点**
- `BTreeMap<PhaseId>` / `BTreeMap<AgentId>`：`PhaseId` 单调、`AgentId` 为 uuid v7（时间可排序），插入序==时间序，**省掉 `indexmap` 依赖**。
- `agent_phase` 索引：`AgentStarted` 携带 `phase_id`，`AgentProgress`/`AgentDone` 不带——首次见到 agent 时登记，后续 O(1) 定位，避免逐 phase 扫描。
- `last_message`：把 `ProgressDelta::Message` 收敛成单行“正在做什么”的活动提示，而非堆积分片（避免刷屏）。

---

## 4. 事件 → 状态：`reduce` 纯函数

覆盖**全部** `AgentEvent` 变体（含 `Pipeline*` 与 `Log`）：

```rust
/// 把一个 AgentEvent 折叠进 AppState。纯函数：不持锁、不 IO。
pub fn reduce(state: &mut AppState, ev: &AgentEvent) {
    use AgentEvent::*;
    match ev {
        RunStarted { task, .. } => {
            state.task_preview = truncate(task, 80);
            state.status = AppStatus::Running;
        }
        PhaseStarted { phase_id, label, planned, .. } => {
            state.phases.entry(*phase_id)
                .or_insert_with(|| PhaseView::new(*phase_id, label, *planned));
        }
        AgentStarted { phase_id, agent_id, prompt_preview, model, .. } => {
            state.agent_phase.insert(*agent_id, *phase_id);
            let p = state.phases.entry(*phase_id).or_insert_with(|| PhaseView::placeholder(*phase_id));
            p.agents.insert(*agent_id, AgentView::running(*agent_id, prompt_preview, model.clone()));
            state.concurrency += 1;
            state.quota_used += 1;
        }
        AgentProgress { agent_id, delta, .. } => {
            if let Some(a) = find_agent_mut(state, agent_id) {
                match delta {
                    ProgressDelta::Tokens { usage } => a.tokens = a.tokens + *usage, // 不进 total
                    ProgressDelta::ToolCall { name, summary } => a.push_tool(name, summary),
                    ProgressDelta::FileEdit { path } => a.push_tool("edit", &path.display().to_string()),
                    ProgressDelta::Message { text } => a.last_message = Some(truncate(text, 80)),
                }
            }
        }
        AgentDone { agent_id, status, tokens, elapsed_ms, .. } => {
            if let Some(a) = find_agent_mut(state, agent_id) {
                a.status = AgentViewStatus::Done(*status);
                a.tokens = *tokens;            // 后端最终统计覆盖增量
                a.elapsed_ms = *elapsed_ms;
            }
            state.total_tokens = state.total_tokens + *tokens;     // total 仅在此累加
            state.concurrency = state.concurrency.saturating_sub(1);
        }
        PhaseDone { phase_id, ok, failed, .. } => {
            if let Some(p) = state.phases.get_mut(phase_id) {
                p.ok = *ok; p.failed = *failed; p.done = true;
            }
        }
        PipelineStarted { total_stages, items, .. } => {
            state.pipelines.push(PipelineView::new(*total_stages, *items));
        }
        PipelineStageStarted { stage_index, label, agents_in_stage, .. } => {
            if let Some(pl) = state.pipelines.last_mut() {
                pl.ensure_stage(*stage_index, label, *agents_in_stage);
            }
        }
        PipelineItemDone { stage_index, status, .. } => {
            if let Some(pl) = state.pipelines.last_mut() {
                pl.record_item(*stage_index, *status);   // items_done++ / ok|failed++
            }
        }
        PipelineDone { stages_completed: _, total_ok, total_failed, .. } => {
            if let Some(pl) = state.pipelines.last_mut() {
                pl.total_ok = *total_ok; pl.total_failed = *total_failed; pl.done = true;
            }
        }
        RunDone { status, total_tokens, .. } => {
            state.status = AppStatus::Done(*status);
            state.total_tokens = *total_tokens;   // 权威值覆盖
            state.concurrency = 0;
        }
        Log { .. } => { /* 日志由 journal forwarder 落盘，不进可视 state（v0.2） */ }
    }
}
```

> **token 计数一致性**（沿用并明确）：`AgentProgress::Tokens` 只更新单 agent 的 `a.tokens`，**不**累加 `total_tokens`；`total_tokens` 仅在 `AgentDone`（增量）与 `RunDone`（权威覆盖）更新，避免分片重复计数。注意 `execute_runtime` 当前发出的 `RunDone` 用 `TokenUsage::default()`（[`cli.rs:306`](../../src/cli.rs#L306)）——在真实 token 计费（roadmap P1-2）接上前，footer 的总数以 `AgentDone` 累加为准；P1-2 完成后 `RunDone` 的权威值才有意义。

---

## 5. 布局与渲染（`render.rs`）

三段式：**header（1–2 行）/ body（可滚动树）/ footer（2 行）**。

```
┌ maestro ─ run 0193f2a1…  ────────────────────────────────────────────────┐
│ task: 审查 src/ 的鉴权与输入校验                        ⏱ 12.3s  ◐ running │
├───────────────────────────────────────────────────────────────────────────┤
│ ▾ [P0] 对抗性验证                                     6 planned · 4✓ 0✗     │
│    ✓ producer#1   sonnet   in 1.2k / out 380     0.8s  “生成 3 条候选…”     │
│  › ◐ adversary#1  opus     read_file · grep      2.1s  投票中…             │
│       ├ read_file  src/auth/mod.rs                                         │
│       └ grep       "verify_token"                                          │
│    · adversary#2  opus     pending                                         │
│ ▸ [P1] 综合报告                                       1 planned             │
│                                                                            │
│ ── pipeline ───────────────────────────────────────────────────────────── │
│ ▾ stage 2/3  summarize                          items 3/3 ✓                │
├───────────────────────────────────────────────────────────────────────────┤
│ tokens 4.2k↑ 1.1k↓  ·  concurrency 2/16  ·  quota 7/1000                   │
│ ↑↓ 选择   ⏎ 展开/折叠   x 停选中 agent   c 取消运行   q 退出               │
└───────────────────────────────────────────────────────────────────────────┘
```

- **header**：`run_id` 短前缀 + `task_preview` + 累计耗时（`started_at.elapsed`）+ 状态徽标（running 转轮 / done 状态色）。
- **body**：phase 列表（折叠时仅一行汇总；展开后逐行 `render_agent_row`：状态图标 + model + 最近工具串 + 耗时 + 活动行/`last_message`，选中行加 `›` 高亮）；agent 展开可显示 `recent_tools`；末尾 `── pipeline ──` 区渲染 `PipelineView`（stage 进度 + items N/M）。
- **footer**：`tokens ↑/↓` · `concurrency N/max_concurrency` · `quota used/limit` + 键位提示；`Cancelling` 态显示转轮“取消中…”。
- **配色/图标**：`Pending ·`(dim) / `Running ◐`(cyan 转轮) / `Ok ✓`(green) / `Error ✗`(red) / `Cancelled ⊘`(grey) / `TimedOut ⌛`(red)。无颜色终端回退到纯符号。
- **滚动**：body 行数超高度时按 `cursor` 保持选中行可见（ratatui `List` + `ListState` 或自管 offset）。

---

## 6. 输入与控制（`input.rs` + `mod.rs`）

```rust
pub enum TuiAction { SelectNext, SelectPrev, ToggleExpand, CancelAgent, CancelRun, Quit }
```

| 键 | 动作 | 落地 |
|----|------|------|
| `↑`/`k`、`↓`/`j` | 移动选择光标 | 纯状态：`state.cursor` 在可见行上增减 |
| `Enter` | 展开/折叠选中的 phase / pipeline | 纯状态：翻转 `expanded` |
| `x` | 取消选中的 agent | `scheduler.cancel_agent(run_id, agent_id)`（[mod.rs:259](../../src/core/scheduler/mod.rs#L259)）；仅对 `Running` 有意义 |
| `c` | 取消整个 run | `run_ctx.cancel.cancel()`（run 级 `CancellationToken`）→ 状态置 `Cancelling`，等 `RunDone(Cancelled)` |
| `q` / `Ctrl-C` | 退出 TUI | 若仍在跑，先 `cancel()` 再退出，避免留下游离执行 |

**为何能停单个 agent**：调度器已提供 `cancel_agent(run_id, agent_id)` 与 `cancel_run(run_id)`（agent token 是 run token 的子节点）。把 `scheduler.clone()` 传入 `run_tui` 即可调用。**暂停不做**：调度器无 pause（§0 非目标）。

---

## 7. 终端生命周期（`mod.rs`）

```rust
fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout()))
}
fn restore_terminal(mut t: Terminal<…>) -> Result<()> {
    disable_raw_mode()?;
    execute!(t.backend_mut(), LeaveAlternateScreen)?;
    t.show_cursor()?; Ok(())
}
```

- **panic 安全**：在进入 raw mode 前安装 panic hook，复原终端后再调用原 hook——否则 panic 会把用户终端留在 raw/alt 屏。
- **保证复原**：`run_app` 用 RAII guard 或在所有返回路径（含 `?` 早退）前 `restore_terminal`；最终 report/错误在**复原后**用普通 `println` 输出（`print_summary`），与 headless 风格一致。

---

## 8. 渲染节奏与背压

```rust
let mut ticker = tokio::time::interval(Duration::from_millis(100));
let mut input = crossterm::event::EventStream::new();   // 需 crossterm "event-stream" feature
loop {
    tokio::select! {
        r = event_rx.recv() => match r {
            Ok(ev) => { let done = matches!(ev, AgentEvent::RunDone { .. });
                        reduce(&mut state, &ev);
                        if done { draw(&mut term, &state)?; break; } }
            Err(broadcast::error::RecvError::Lagged(n)) => state.lagged += n, // 仅提示，不致命
            Err(broadcast::error::RecvError::Closed) => break,
        },
        _ = ticker.tick() => { state.spinner += 1; draw(&mut term, &state)?; } // 合并突发，100ms 刷新
        Some(Ok(crossterm::event::Event::Key(k))) = input.next() => {
            match input::map_key(k) {
                TuiAction::Quit       => { run_ctx.cancel.cancel(); break; }
                TuiAction::CancelRun  => { run_ctx.cancel.cancel(); state.status = AppStatus::Cancelling; }
                TuiAction::CancelAgent=> if let Some(a) = state.selected_agent() { scheduler.cancel_agent(run_ctx.run_id, a); }
                act => input::apply_nav(&mut state, act),  // 选择/折叠后立即 draw
            }
            draw(&mut term, &state)?;
        }
    }
}
```

- **更新与渲染解耦**：每个事件都 `reduce`（廉价），但**只在 100ms tick / 按键 / RunDone 时重绘**，合并事件突发，避免高频闪烁与重绘开销。
- **背压**：`broadcast` 容量 256（[`cli.rs:241`](../../src/cli.rs#L241)）。慢消费导致 `Lagged(n)` 时只累计提示计数、不退出——可视状态允许丢中间帧（终态由 `AgentDone`/`RunDone` 校正）。
- **退出条件**：见到 `RunDone`（绘最后一帧后 break）/ 流 `Closed` / 用户 `q`。

---

## 9. `maestro workflows` 列表视图

P2-1 的第二半：`maestro workflows` 列出运行 + 进入进度视图。

- **列表**：复用 [`list_runs_cmd`](../../src/cli.rs#L116)（按 `updated_at` 倒序的 `StatusOutput`），用 ratatui `List` 渲染 `run_id · task · status · tokens · 时间`，↑↓ 选择、`Enter` 进详情、`q` 退出。
- **详情（只读）**：读 `checkpoint.json` + `events.jsonl`，用 §4 的 `reduce` **回放**历史事件重建 `AppState`，复用 §5 渲染——一套渲染器同时服务实时与回放。
- **未决（live-attach）**：附着到**另一个进程**正在进行的 run 需要把事件总线跨进程暴露（IPC / 重读增长中的 `events.jsonl` 做 tail-follow）。v0.2 先支持“只读快照 + 手动刷新”，真正的跨进程实时附着随 [roadmap P2-3 后台 run](../roadmap-p1-p2.md) 一并设计。

---

## 10. 依赖

```toml
ratatui   = "0.29"
crossterm = { version = "0.28", features = ["event-stream"] }   # EventStream 需此 feature
```

`futures = "0.3"`（已在 [Cargo.toml](../../Cargo.toml#L32)）提供 `StreamExt::next`。无需 `indexmap`（§3 用 `BTreeMap`）。

---

## 11. 测试

- **`reduce` 单测**（纯函数、确定性）：构造事件序列断言 `AppState`——
  - phase 计数：`PhaseStarted(planned=3)` + 3×`AgentStarted` → `concurrency==3`、`quota_used==3`；3×`AgentDone(Ok)` → `concurrency==0`、`phase.ok` 经 `PhaseDone` 置位。
  - token 一致性：`AgentProgress::Tokens` 不动 `total_tokens`；`AgentDone` 累加；`RunDone` 覆盖。
  - pipeline：`PipelineStarted/StageStarted/ItemDone×N/Done` → `stages`/`items_done`/`total_ok`。
  - 路由：`AgentProgress` 仅凭 `agent_id` 经 `agent_phase` 命中正确 phase。
- **渲染 golden 帧**：`ratatui::backend::TestBackend` 渲染到 buffer，断言关键单元格/快照（CI 稳定，无真实终端）。
- **输入映射**：`map_key` / `apply_nav` 表驱动单测（选择越界 clamp、折叠翻转）。

---

## 12. 相对 `design/cli.md` §8.5 的变更

| 项 | §8.5（旧） | 本设计 |
|----|-----------|--------|
| Pipeline 事件 | ❌ 未覆盖 | ✅ `PipelineView`/`StageView` + reduce 分支 |
| 容器类型 | `IndexMap`（需依赖） | `BTreeMap`（利用 PhaseId 单调 + AgentId v7 时序，零新依赖） |
| 暂停 | `TogglePause`→`TuiControl::Pause` | ❌ 移除（调度器无 pause 原语） |
| 停单 agent | 假设 `control_tx`/`TuiControl` | ✅ 真实 `Scheduler::cancel_agent` |
| 取消整 run | `TuiControl::StopRun` | ✅ `RunContext::cancel`（已存在） |
| Message 分片 | “v0.1 不展示” | ✅ 收敛为 `last_message` 活动行 |
| agent→phase 路由 | `find_agent` 扫描 | ✅ `agent_phase` 索引 O(1) |
| 集成 | 抽象 `start_tui(...)` | ✅ 对齐现有 `run_tui`/`execute_runtime` 的 spawn 并发模型 |

---

## 13. 未决问题 / 非目标

1. **pipeline 内部 agent 的归并**：`pipeline()` 的 handler 内部 `agent()` 也会广播 `AgentStarted/AgentDone`，但事件未携带其与某个 stage/item 的关联——当前会落到默认 phase。**待定**：给 pipeline 内 agent 事件加 stage 元数据，或在 TUI 中将其折叠进 `PipelineView`。
2. **`RunDone` 的 token 权威值**：`execute_runtime` 现发 `TokenUsage::default()`；依赖 roadmap P1-2 接入真实计费后才权威，目前以 `AgentDone` 累加为准（§4 注）。
3. **跨进程 live-attach**（§9）：随 P2-3 后台 run 设计。
4. **窄终端/超长 phase**：滚动与折行的极端布局留待实现期打磨。

## 14. 相关文档

- **交互/UX 配套：** [design/tui-interaction.md](./tui-interaction.md) — 设计原则 / 导航地图 / 视觉语言 / 键位 / 流程 / 边界态（本文管“怎么实现”，它管“用户看到什么、怎么操作”）
- 现状与桩：[architecture/cli.md §4/§7](../architecture/cli.md)
- 事件契约：[`event.rs`](../../src/core/contract/event.rs) · [architecture/core.md](../architecture/core.md)
- 调度取消：[`scheduler/mod.rs`](../../src/core/scheduler/mod.rs)（`cancel_run`/`cancel_agent`）
- 路线图：[roadmap-p1-p2.md P2-1](../roadmap-p1-p2.md)
- 旧草图（被取代）：[design/cli.md §8.5](./cli.md)
