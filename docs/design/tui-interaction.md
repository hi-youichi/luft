# Maestro TUI 交互设计（P2-1·UX 规格）

> **状态：设计稿（未实现）。** 本文是 [`design/tui.md`](./tui.md)（实现设计）的**交互/UX 配套规格**：tui.md 回答“**怎么实现**”（状态模型、`reduce`、模块、渲染管线），本文回答“**用户看到什么、怎么操作、为什么这样**”。两者面向不同读者——tui.md 给实现者，本文给设计评审与实现者对齐交互细节。
>
> 对齐当前代码：事件契约 [`event.rs`](../../src/core/contract/event.rs)、状态/取消 [`scheduler/mod.rs`](../../src/core/scheduler/mod.rs)、命令面 [`main.rs`](../../src/main.rs)、当前桩 [`cli.rs run_tui`](../../src/cli.rs#L416)。

---

## 0. 范围

**本文覆盖（交互层）：** 设计原则 · 用户与场景 · 信息架构 · 屏幕与导航地图 · 实时视图解剖 · 交互模型与键位 · 视觉语言 · 状态与反馈 · 关键流程逐步拆解 · 边界/异常/窄屏 · 可访问性 · 文案 · 组件清单。

**本文不覆盖（见 tui.md）：** `AppState` 结构、`reduce` 折叠逻辑、模块切分、渲染节奏的并发实现、终端生命周期代码、单测策略。

**非目标（沿用 tui.md §0）：** 暂停/恢复（调度器无 pause 原语）、跨进程 live-attach、鼠标、主题配置。本文在交互上对这些点给出**降级表现**而非功能。

---

## 1. 设计原则

> 排在前面的优先级更高；冲突时上压下。

1. **实时透明 > 完整。** 用户首要诉求是“现在到哪了、卡没卡、烧了多少”。一屏内回答 *what now / is it stuck / how much*，宁可折叠细节也不堆信息。
2. **可控、可中止、可解释。** 每个可见实体（run / phase / agent）都能被定位；破坏性操作（取消）必须可被理解且对“整 run”级别要二次确认。无声的不可逆操作是反模式。
3. **低噪音、抗刷屏。** Agent 的流式 `Message` 分片**收敛成单行活动提示**，不堆历史；高频事件按节奏合并重绘（见 §8）。视图是“状态投影”，不是日志滚动条。
4. **一套渲染、双重来源。** 实时（事件流）与回放（`events.jsonl`）共用同一视图与同一信息架构——用户在 live 学到的心智模型，在历史 run 里**零成本复用**。
5. **终端原生、键盘优先。** 不假设鼠标、不假设真彩色、不假设宽屏。所有功能键盘可达；所有颜色有纯符号回退。
6. **失败是一等状态。** `Error / TimedOut / Cancelled / Partial` 不是边角，要有明确的图标、配色与汇总位，且不被“成功流”淹没。

---

## 2. 用户与场景（Jobs-to-be-done）

| 角色 | 场景 | 进入时最想立刻知道 |
|------|------|----------------------|
| **工作流作者** | 刚 `maestro run` 一个新写的 `.lua`，盯着跑 | 调度对不对？phase/agent 的并发与扇出符合预期吗？哪个 agent 在干活 |
| **运维/重跑者** | 一个长 run（research/pipeline）在后台跑，回来看进度 | 跑完没？失败了几个？token/quota 烧到哪了？要不要取消 |
| **排查者** | 某个 run 失败，事后看 `maestro workflows` 回放 | 哪一步崩的？崩之前那个 agent 调了什么工具、说了什么 |
| **演示者** | 给别人看“多 agent 并行”的实时编排 | 画面是否清晰呈现 phase→agent→tool 的层级与并发 |

**贯穿性诉求（每个角色都要）：** 进度（phase/agent 状态）、并发度、累计耗时、token/quota、以及“出问题时的那一行”。这四类信息决定了 §6 footer 的固定常驻内容。

---

## 3. 信息架构

工作流原语（[`parallel`](../../examples/parallel-demo.lua)/`converge`/[`pipeline`](../../examples/pipeline-demo.lua)）在事件流里折叠成如下层级。**这是用户的心智模型，所有屏幕都围绕它组织：**

```
run                      一次执行（run_id · task）
├─ phase                 一个顶层 parallel/converge → 一个 phase（label · planned 数）
│   └─ agent             一次 agent() 调用（status · model · tokens · 耗时）
│        └─ tool / edit  agent 的工具调用 / 文件编辑（最近 N 条）
│        └─ 活动行        最近一条 Message，收敛为“正在做什么”
└─ pipeline              一个 pipeline() → 多 stage 流式管道
    └─ stage             一个阶段（label · items N/M · ok/failed）
```

**层级 → 可见性映射（默认折叠态）：**

| 层级 | 折叠时 | 展开时（Enter） |
|------|--------|------------------|
| phase | 一行汇总：`label · N planned · ok✓ failed✗` | 逐 agent 行 |
| agent | 一行：状态 + model + 最近工具串 + 耗时 + 活动行 | 追加 `recent_tools` 明细（容量 5） |
| pipeline | 一行：`stage i/N · label · items N/M` | 逐 stage 进度 |

**取舍：** 默认**phase 展开、agent 折叠**——让用户先看到“有哪些 agent、谁在跑”，工具明细按需下钻，避免初始即刷屏。

---

## 4. 屏幕与导航地图

三块屏幕，共用同一棵树渲染器（原则 4）：

```
              maestro run …                      maestro workflows
                  │                                     │
                  ▼                                     ▼
         ┌─────────────────┐   q(运行中→确认)   ┌──────────────────┐
         │  ① 实时运行视图  │◄───────────────── │  ② Run 列表       │
         │   (Live)        │                   │  (Workflows)     │
         └────────┬────────┘                   └───────┬──────────┘
              RunDone                              Enter│ ▲ q/Esc
                  │ 复原终端后打印 summary              ▼ │
                  ▼                              ┌──────────────────┐
            普通屏：最终 report                   │  ③ Run 回放详情   │
                                                 │  (Replay 只读)   │
                                                 └──────────────────┘
```

- **① 实时运行视图（Live）：** `maestro run` 的主屏。事件流 → 实时树。退出 = `RunDone` 自然收尾，或 `q`（运行中需确认，见 §10.2）。详见 §5–§6。
- **② Run 列表（Workflows）：** `maestro workflows`。读各 run 的 `checkpoint.json`，列表呈现。`↑↓` 选择、`Enter` 进③、`q` 退出。
- **③ Run 回放详情（Replay·只读）：** 读 `events.jsonl` 用同一 `reduce` 回放重建状态，复用①的树渲染。**只读**：无取消键，footer 标 `replay`。

> **跨进程 live-attach 留白：** 附着到“另一个进程正在跑”的 run 需要跨进程事件总线（见 tui.md §9）。v0.2 在②③只提供“只读快照 + 手动 `r` 刷新”，交互上以 footer 文案 `snapshot · r 刷新` 明确告知“非实时”，避免用户误以为画面会自己动。

---

## 5. 实时运行视图：解剖

三段式 + 可选 pipeline 区。固定 **header（1–2 行）/ body（可滚动树）/ footer（2 行）**。

```
┌ maestro ─ run 0193f2a1…  ───────────────────────────────────────────────────┐  ← header
│ task: 审查 src/ 的鉴权与输入校验                          ⏱ 12.3s   ◐ running │
├──────────────────────────────────────────────────────────────────────────────┤
│ ▾ [P0] 对抗性验证                                      6 planned · 4✓ 0✗      │  ← body: phase 行
│    ✓ producer#1   sonnet   in 1.2k / out 380      0.8s  "生成 3 条候选…"      │     agent 行(折叠)
│  › ◐ adversary#1  opus     read_file · grep        2.1s  投票中…             │     ← 选中(›)+running
│       ├ read_file  src/auth/mod.rs                                            │     ← 展开:recent_tools
│       └ grep       "verify_token"                                            │
│    · adversary#2  opus     pending                                           │     ← pending(dim)
│ ▸ [P1] 综合报告                                        1 planned              │     ← 折叠 phase(▸)
│                                                                              │
│ ── pipeline ────────────────────────────────────────────────────────────────│  ← pipeline 区
│ ▾ stage 2/3  summarize                            items 3/3 ✓                │
├──────────────────────────────────────────────────────────────────────────────┤
│ tokens 4.2k↑ 1.1k↓  ·  concurrency 2/16  ·  quota 7/1000           follow ●  │  ← footer 行1: 度量
│ ↑↓ 选择   ⏎ 展开/折叠   x 停选中   c 取消运行   f 跟随   q 退出               │  ← footer 行2: 键位
└──────────────────────────────────────────────────────────────────────────────┘
```

### 区域职责

| 区域 | 内容 | 交互/动态 |
|------|------|-----------|
| **header L1** | `maestro ─ run <短id>` + 累计耗时 `⏱` + 状态徽标 | 徽标随状态变色/转轮：`◐ running`(cyan) / `✓ done`(green) / `⚠ partial`(yellow) / `✗ failed`(red) / `⊘ cancelling…`(grey) |
| **header L2** | `task:` + 任务预览（截断 ~80 字） | 静态，来自 `RunStarted.task` |
| **body** | phase 树 + agent 行 + pipeline 区，按 §3 折叠规则 | 唯一可滚动/可选区；`cursor` 始终保持可见（§11） |
| **footer L1** | `tokens ↑/↓` · `concurrency N/max` · `quota used/limit` · `follow` 指示 | 常驻四指标（§2）；`max=16`、`limit=1000` 来自 `SchedulerConfig` 默认 |
| **footer L2** | 当前可用键位提示 | **随上下文变化**：列表屏/回放屏/确认态显示不同键位 |

### agent 行的列结构（折叠态）

```
 ›  ◐   adversary#1     opus       read_file · grep          2.1s   "投票中…"
 │  │   │               │          │                          │      │
 选中 状态  名/序号        model      最近工具串(最新在右)        耗时    活动行(last_message)
```

- **活动行**是把 `ProgressDelta::Message` 收敛后的“正在做什么”单行，**永远只显示最新一条**（原则 3）。无 message 时留空，不显示占位噪音。
- 列宽自适应：窄屏按优先级丢列（活动行 → 工具串 → model），见 §11。

---

## 6. 交互模型

### 6.1 焦点与选择

- **单一线性光标**在 body 的“可见行”上移动（折叠状态决定哪些行可见）。选中行以 `›` 前缀 + 反色高亮。
- **焦点层级**由行类型隐含：phase 行 / agent 行 / pipeline 行。操作（如 `x` 取消）只对“当前选中行所代表的实体”生效，且对不适用的行类型**无操作 + 一次性 footer 提示**（如对已 Done 的 agent 按 `x` → footer 闪 `该 agent 已结束`）。

### 6.2 展开/折叠

- `Enter`：翻转当前行的展开态（phase / agent / pipeline 均可展开）。
- `→` / `l`：展开（已展开则下钻进首个子行）；`←` / `h`：折叠（已折叠则跳到父行）。tree 风格，给 vim 用户与方向键用户各一套。

### 6.3 跟随模式（follow）— 实时视图的核心交互

直播日志类界面的关键体验。

- **默认开启 follow**：光标自动跟随“最新活动”（最近 `AgentStarted` / 有 `Message` 的 running agent），新事件来时视图自动滚到底部保持其可见。footer 显示 `follow ●`。
- **任何手动导航**（`↑↓←→/jk/Enter`）**自动关闭 follow**，把光标交给用户，footer 变 `follow ○`，画面不再自动跳——用户可以安心看历史 agent 而不被新事件抢走视线。
- `f`：手动切换 follow；重新开启时光标跳回当前最新活动。

> 这是“可控 vs 实时”的平衡点（原则 1+2）：实时默认不打扰，一旦用户介入就让位。

### 6.4 键位总表

| 键 | 屏幕 | 动作 | 行为 |
|----|------|------|------|
| `↑`/`k`、`↓`/`j` | 所有 | 移动光标 | 关闭 follow；在可见行上增减、越界 clamp |
| `←`/`h`、`→`/`l` | 所有树屏 | 折叠 / 展开 | tree 导航（§6.2） |
| `Enter` | Live/Replay | 展开折叠选中行；列表屏=进详情 | 翻转 `expanded` / 进③ |
| `g` / `G` | 树屏 | 跳到顶 / 底 | 便于长 run 快速定位 |
| `f` | Live | 切换跟随 | §6.3 |
| `x` | **仅 Live** | 取消**选中的 agent** | `scheduler.cancel_agent(run_id, id)`；仅对 `Running` 有意义，否则提示 |
| `c` | **仅 Live** | 取消**整个 run** | **二次确认**（§10.2）→ `run_ctx.cancel.cancel()` → `Cancelling` |
| `r` | Workflows/Replay | 刷新快照 | 重读 checkpoint / events.jsonl |
| `q` / `Esc` | 所有 | 返回 / 退出 | Live 运行中：先确认（§10.2）；其余直接退 |
| `Ctrl-C` | 所有 | 强制退出 | 运行中先 `cancel()` 再退，避免游离执行 |

**破坏性操作的二次确认**是本文相对 tui.md §6 的**交互细化**：tui.md 把 `c` 设为即时取消；交互上“取消整 run”不可逆且代价大，应内联确认（footer 变 `确认取消整个 run？ y/n`），而 `x` 取消单 agent 代价小、可重跑，保持即时。

### 6.5 无模态（modal-less）

不弹独立对话框。所有“模态”交互（确认、提示、刷新中）都**就地占用 footer L2**，按完即恢复键位提示。理由：终端弹层易错位、抢焦点；footer 行内确认保持上下文不丢失。

---

## 7. 视觉语言

### 7.1 状态图标 + 配色（单一事实源）

映射 `AgentStatus` / `RunStatus`，全屏复用：

| 状态 | 图标 | 颜色 | 无色回退 | 来源 |
|------|------|------|----------|------|
| Pending | `·` | dim/grey | `·` | 尚未 `AgentStarted` |
| Running | `◐` 转轮 | cyan | `◐`（仍转轮） | `AgentStarted`，未 `Done` |
| Ok | `✓` | green | `✓` | `AgentStatus::Ok` |
| Error | `✗` | red | `✗` | `AgentStatus::Error` |
| Cancelled | `⊘` | grey | `⊘` | `AgentStatus::Cancelled` |
| TimedOut | `⌛` | red | `⌛` | `AgentStatus::TimedOut` |
| Partial（run 级） | `⚠` | yellow | `⚠` | `RunStatus::Partial` |

**配色规则：** 颜色**永不单独承载语义**（可访问性，§12）——图标符号本身已可区分，颜色仅作强化。检测到 `NO_COLOR` 或非 tty 时整屏降级为纯符号，布局不变。

### 7.2 转轮（spinner）

- 仅 `Running` 实体与 header `running`/`cancelling` 徽标转动，帧序 `◐◓◑◒`，由 100ms tick 推进（§8）。静态状态不动，避免“假装在忙”。

### 7.3 排版与对齐

- agent 行**列对齐**（状态｜名｜model｜工具｜耗时｜活动行），靠空格 pad 成列，扫读时同类信息纵向对齐。
- **数值人性化：** token `4.2k↑ 1.1k↓`（千分位缩写 + 方向箭头），耗时 `0.8s` / `2.1s` / `1m04s`，避免裸 ms/裸大数。
- **截断：** task ~80 字、prompt 预览 ~60 字、活动行 ~80 字，统一尾部 `…`。截断发生在“能看清是什么”的边界，不在词中胡乱切。
- **Unicode 优先、ASCII 兜底：** 框线/图标用 Unicode；`TERM` 不支持时回退 `+-|` 框线与 ASCII 图标（`*` running、`v`/`>` 展开折叠）。

### 7.4 折叠指示

`▾`=展开 / `▸`=折叠（phase、pipeline、可展开 agent 行首）。无子项的 agent 行不显三角，避免误导可下钻。

---

## 8. 状态与反馈

### 8.1 实体生命周期（用户可见的状态机）

```
agent:   pending(·) ──AgentStarted──► running(◐) ──AgentDone──► ok(✓)/error(✗)/cancelled(⊘)/timedout(⌛)
phase:   (隐含)running ─────────────────────────► PhaseDone ──► 汇总 ok✓ failed✗
run:     running(◐) ──[c/x 取消]──► cancelling(⊘…) ──RunDone──► done(✓)/partial(⚠)/failed(✗)/cancelled(⊘)
```

### 8.2 反馈节奏（来自 tui.md §8，交互含义）

- **更新与重绘解耦：** 每个事件即时折叠进状态（廉价），但**只在 100ms tick / 按键 / RunDone 时重绘**。用户感知：画面平滑（~10fps）不闪烁，按键**零延迟**响应（按键单独触发重绘）。
- **耗时实时走表：** running agent 的耗时与 header `⏱` 由 tick 驱动持续增长，给“它确实在动”的体感（即使没有新事件）。

### 8.3 特殊反馈

| 情形 | 触发 | 表现 |
|------|------|------|
| **事件积压/丢帧** | `broadcast::Lagged(n)` | footer 角标 `⚠ 丢 n 帧`，不致命（终态由 `AgentDone`/`RunDone` 校正）。诚实告知“中间帧没了”，而非假装完整 |
| **取消中** | 按 `c` 确认后 | header `⊘ cancelling…` 转轮，等 `RunDone(Cancelled)`；期间禁用再次 `c`，避免重复 |
| **疑似卡住** | running agent 超过 ~30s 无任何 `AgentProgress` | 活动行追加灰字 `· 静默 32s`，提示用户“可能卡了，可按 `x`” |
| **无操作可用** | 对不适用行按 `x`/`c` | footer 一次性闪提示（如 `该 agent 已结束`），1.5s 后恢复 |

---

## 9. 空 / 加载 / 结束 态

| 态 | 何时 | 画面 |
|----|------|------|
| **启动中** | 已进 TUI，未收到首个事件 | body 居中 `等待调度…`（dim）+ header 转轮，footer 仅 `q 退出` |
| **空 run** | 工作流没起任何 phase/agent 即结束 | body `（本次运行无 agent）`，直接进结束态 |
| **结束（live）** | 收到 `RunDone` | 绘最后一帧；header 定格终态徽标；footer 变 `运行结束 · q 退出 / Enter 看报告`。**不自动清屏**，让用户看完最后状态 |
| **退出后** | 复原终端、回普通屏 | 用 `print_summary` 打印最终 report/错误（与 headless 风格一致），TUI 不负责长报告滚动 |
| **列表为空** | `maestro workflows` 无历史 run | `还没有运行记录 · maestro run <prompt> 开始`（带下一步引导，不是死胡同） |

> **结束不抢断：** 即使 `RunDone` 到达，也保留树视图供用户最后扫一眼失败项，由用户主动 `q`/`Enter` 离开——而非立刻跳走。

---

## 10. 关键流程逐步拆解

### 10.1 下钻查看一个 agent 调了什么工具

1. `↓` 到目标 agent 行（follow 自动关闭，画面停住）。
2. `Enter` 或 `→` 展开 → 行下方出现 `recent_tools`（最近 5 条，最新在前）：
   ```
     › ◐ adversary#1  opus   read_file · grep   2.1s  "投票中…"
          ├ read_file  src/auth/mod.rs
          └ grep       "verify_token"
   ```
3. `←` 折叠收起。`f` 跳回最新活动并恢复 follow。

### 10.2 取消整个 run 并安全退出

1. 按 `c`。footer L2 就地变：`确认取消整个 run？ y 确认 / n 取消`（无独立弹窗，§6.5）。
2. `n`/`Esc` → 取消该确认，footer 恢复键位，run 继续。
3. `y` → `run_ctx.cancel.cancel()`；header → `⊘ cancelling…`；运行中的 agent 陆续变 `⊘`；`c` 暂禁用。
4. 收到 `RunDone(Cancelled)` → 进结束态，footer `已取消 · q 退出`。
5. （快捷）运行中直接按 `q`：等价“先确认取消、确认后 `cancel()` 再退”，**绝不**留下游离执行（`Ctrl-C` 同此保障）。

### 10.3 停掉某个跑飞的 agent（不动其余）

1. `↓` 定位到该 running agent。
2. `x` → `scheduler.cancel_agent(run_id, agent_id)`（agent token 是 run token 子节点，只杀这一个）。**无需二次确认**（代价小、可重跑）。
3. 该行变 `⊘ cancelled`；`concurrency` 减 1；其余 agent/phase 不受影响，run 继续。

### 10.4 事后回放一个失败的 run

1. `maestro workflows` → ② 列表（按 `updated_at` 倒序）：
   ```
   › 0193f2a1…  审查 src/ 鉴权与输入校验      ⚠ partial   12.4k tok   2m 前
     0193e0bc…  深度研究: Rust async runtime  ✓ done      48.1k tok   1h 前
   ```
2. `↑↓` 选中失败 run，`Enter` → ③ 回放详情：`reduce` 重放 `events.jsonl` 重建状态，**同一棵树**呈现。
3. 树里 `✗`/`⌛` 行一眼可见；`Enter` 展开看崩溃前 agent 的工具串与最后活动行。
4. footer 标 `replay · 只读`，**无 `x`/`c`**；`q`/`Esc` 回列表，`r` 重读快照。

---

## 11. 边界、异常与响应式

| 情形 | 交互表现 |
|------|----------|
| **body 超出高度** | 按 `cursor` 自动滚动保持选中行可见；follow 态钉在底部。顶/底以 `↑ 还有 N 行` / `↓ 还有 N 行` 提示，不显滚动条 |
| **窄终端（<80 列）** | 按优先级**逐列丢弃**：活动行 → 工具串 → model，保留 `状态·名·耗时`。极窄（<50）只留 `状态 名`。绝不横向截断关键状态 |
| **超长 phase（上百 agent）** | 折叠态只占一行；展开后靠滚动 + `g/G` + （未来）`/` 过滤导航。phase 汇总行常驻 `4✓ 0✗ · 96 running` 让用户不展开也知全貌 |
| **超矮终端（<10 行）** | header 压成 1 行（合并 run+task）、footer 压成 1 行（仅最关键键位），body 至少留 3 行 |
| **大量并发刷新** | 重绘节流（§8.2）吸收突发；`Lagged` 只提示不退出 |
| **panic** | 进 raw mode 前装 panic hook，**先复原终端**再打印 panic——用户终端绝不留在 raw/alt 花屏（实现见 tui.md §7，UX 上即“崩了也还你一个干净终端”） |
| **非 tty / 管道** | 不进 TUI（无意义）；交互上由 CLI 层回退 headless，本视图不强行渲染 |

---

## 12. 可访问性

- **不靠颜色单独表意**（§7.1）：每个状态都有独立符号；色盲/单色终端语义不丢。配色选 green/red 之外辅以形状差异（`✓` vs `✗`）。
- **`NO_COLOR` / `TERM=dumb`：** 整屏纯符号 + ASCII 框线，布局与键位不变。
- **键盘全可达：** 无任何鼠标依赖功能；所有操作有方向键与 vim 键双映射。
- **屏幕阅读器：** TUI 实时画面对 SR 不友好是终端固有限制——因此**最终 report/错误在复原后用普通 stdout 输出**（§9），SR 用户可读到结构化结果；headless（JSONL）作为完全无障碍的替代路径。
- **不闪烁：** ~10fps 节流重绘、无快速颜色翻转，避免光敏不适。

---

## 13. 文案与本地化

- **语气：** 简洁、动词开头、终端腔。键位提示用「图标 + 动词」：`⏎ 展开/折叠`、`c 取消运行`。
- **状态词统一：** running/pending/done/partial/failed/cancelled/timedout 全屏一致用词，与 §7.1 图标一一对应，不混用近义词。
- **数字本地化：** 千分位缩写（`4.2k`）、相对时间（`2m 前`/`1h 前`）、耗时（`0.8s`/`1m04s`）。
- **i18n：** 文案集中常量化，便于中英切换；当前文档以中文为基线（与现有 docs 一致）。截断长度按字符（非字节）计，避免切坏多字节。

---

## 14. 组件清单（可复用 UI 单元）

供 tui.md `render.rs` 落地参照；每个都是纯 `state → 视觉` 映射：

| 组件 | 职责 | 出现处 |
|------|------|--------|
| `StatusBadge` | 状态图标+色+转轮，含无色回退 | header、agent 行、列表、回放 |
| `RunHeader` | run 短 id + task + `⏱` + 徽标，可压缩成 1 行 | Live/Replay header |
| `PhaseRow` | phase 汇总行（折叠三角 + label + planned + ok/failed） | body |
| `AgentRow` | 对齐列：状态｜名｜model｜工具串｜耗时｜活动行，列可按宽度丢弃 | body |
| `ToolList` | 展开态 `recent_tools` 明细（容量 5） | body |
| `PipelineBar` | stage 进度 + `items N/M` + ok/failed | pipeline 区 |
| `MetricsFooter` | tokens↑↓ · concurrency N/max · quota used/limit · follow | footer L1 |
| `KeyHints` | 上下文相关键位/确认/提示行 | footer L2 |
| `Spinner` | 100ms tick 帧推进 | StatusBadge 内 |
| `ScrollHint` | `↑/↓ 还有 N 行` 边缘提示 | body 顶/底 |
| `EmptyState` | 等待/空/无记录引导文案 | 各屏空态 |

---

## 15. 相对 tui.md 的交互细化与开放问题

**本文新增/细化（相对 tui.md §5/§6 的交互层补强）：**

| 点 | tui.md | 本文细化 |
|----|--------|----------|
| 取消整 run | `c` 即时 `cancel()` | **二次确认**内联 footer（§6.4/§10.2），破坏性操作不静默 |
| 跟随实时 | 未定义 | **follow 模式**（§6.3）：手动导航即让位，平衡实时与可控 |
| 退出语义 | `q` 先 cancel 再退 | 明确为“先确认取消”流程（§10.2） |
| 卡死感知 | 无 | running 静默 >30s 的 `· 静默 Ns` 提示（§8.3） |
| 窄屏 | “留待打磨” | 按优先级**逐列丢弃**的明确降级序（§11） |
| 空/结束态 | 未定义 | 等待/空/结束/空列表的完整态与引导文案（§9） |
| 树导航 | `↑↓/Enter` | 增 `←→/hjkl/g/G` tree 导航（§6.2） |

**开放 UX 问题（待实现期或后续迭代定）：**

1. **pipeline 内 agent 的归并：** `pipeline()` handler 内的 `agent()` 也发 `AgentStarted/Done` 但不带 stage 关联，当前会落到默认 phase（见 tui.md §13.1）。交互上这些 agent 该显示在 pipeline 区某个 stage 下，还是单列？依赖事件加 stage 元数据。
2. **过滤/搜索：** 长 run 是否需要 `/` 过滤（仅看 failed / 某 model）？本期未做，列为后续。
3. **`RunDone` token 权威值：** 现为 `TokenUsage::default()`（[`cli.rs:360`](../../src/cli.rs#L360)）；footer 总数在 P1-2 真实计费接入前以 `AgentDone` 累加为准（见 tui.md §13.2）。交互上需保证“运行中累加”与“结束权威值”不出现跳变突变（若有差额，footer 不闪烁、平滑覆盖）。
4. **回放的时间轴：** ③ 是否提供“按事件步进/时间刻度”回放，而非一次性重建？本期只做终态快照，时间轴回放列为后续。

---

## 16. 相关文档

- **实现设计（配套）：** [`design/tui.md`](./tui.md) — 状态模型 / `reduce` / 模块 / 渲染管线 / 终端生命周期 / 测试
- 事件契约：[`event.rs`](../../src/core/contract/event.rs) · 状态/取消：[`scheduler/mod.rs`](../../src/core/scheduler/mod.rs)（`cancel_run`/`cancel_agent`、`max_concurrency`/`quota_per_run` 默认）
- 命令面：[`main.rs`](../../src/main.rs)（`run`/`workflows`/`list`/`status`/`logs`）· 当前 TUI 桩：[`cli.rs run_tui`](../../src/cli.rs#L416)
- 工作流原语示例：[`parallel-demo.lua`](../../examples/parallel-demo.lua) · [`pipeline-demo.lua`](../../examples/pipeline-demo.lua) · [`deep-research.lua`](../../examples/deep-research.lua)
- CLI 架构与现状：[`architecture/cli.md`](../architecture/cli.md)（§7 标注 TUI 为文本桩）
