# Web UI 设计

> 为 Maestro 设计一个基于浏览器的实时控制台，通过 [`maestro serve`](websocket-server.md) 暴露的 WebSocket 接口提交 run、订阅事件、查看 findings 与报告。
>
> **设计目标**：复用 WebSocket 协议的全部能力，不要求服务端新增任何消息类型；视觉与交互语义与 [TUI 设计](tui/README.md) 保持一致（同一套状态图标、信息层级、用词），使 Web / TUI / headless 三种前端共享同一心智模型。

---

## 目录

1. [背景与目标](#1-背景与目标)
2. [整体架构](#2-整体架构)
3. [信息架构与屏幕地图](#3-信息架构与屏幕地图)
4. [视觉语言](#4-视觉语言)
5. [核心界面规格](#5-核心界面规格)
6. [WebSocket 客户端层](#6-websocket-客户端层)
7. [状态模型](#7-状态模型)
8. [关键用户流程](#8-关键用户流程)
9. [实时事件处理](#9-实时事件处理)
10. [错误与边界状态](#10-错误与边界状态)
11. [技术选型与依赖](#11-技术选型与依赖)
12. [实现计划](#12-实现计划)
13. [已知局限与后续工作](#13-已知局限与后续工作)

---

## 1. 背景与目标

[`websocket-server.md`](websocket-server.md) 为 Maestro 增加了面向程序的长连接接口。Web UI 是它的第一个一等公民客户端，面向以下场景：

- **本地开发者**：在浏览器里提交 NL/workflow run，看 phase→agent 树实时推进，比终端更易展示长文本 prompt、报告 markdown、findings 表格。
- **NL run 审查**：利用 `confirm: true` 流程，在执行前以语法高亮预览生成的 Lua 脚本，确认后再跑。
- **历史回看**：浏览 `list_runs`，对已完成的 run 用 `get_logs` + `get_report` + `get_findings` 重建只读详情页。

设计约束：

- **服务端零改动**：Web UI 仅消费 [§3 消息协议](websocket-server.md#3-消息协议) 已定义的消息。任何"如果服务端能多发一个字段就好了"的诉求记入 [§13](#13-已知局限与后续工作)，不在 v0.1 假设其存在。
- **语义一致**：状态图标、颜色、用词、信息层级直接映射 [TUI 03-visual-language](tui/03-visual-language.md) 与 [02-information-architecture](tui/02-information-architecture.md) 的单一事实源。
- **单连接多 run**：一条 WebSocket 连接即可订阅多个 run（协议本身支持），UI 用标签/列表管理并行 run。
- **可降级**：连接断开、broadcast lagged、run 已完成等情形都有明确 UI 反馈，且能通过 `get_logs` 补齐。

非目标（v0.1 不做）：多用户协作、服务端持久化 UI 偏好、移动端布局、鉴权 UI（服务端 v0.1 也未实现鉴权）。

---

## 2. 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│  Browser (SPA)                                               │
│                                                             │
│  ┌────────────┐   ┌──────────────┐   ┌──────────────────┐  │
│  │  UI 组件层  │◀─▶│  Store        │◀─▶│  WsClient        │  │
│  │  React     │   │  (run 树/     │   │  连接/重连/      │  │
│  │  视图      │   │   findings/   │   │  req-id 关联/    │  │
│  │            │   │   连接状态)   │   │  事件分发        │  │
│  └────────────┘   └──────────────┘   └────────┬─────────┘  │
└──────────────────────────────────────────────┼────────────┘
                                                │ ws://127.0.0.1:7474/ws
                                                ▼
                          maestro serve  (见 websocket-server.md)
```

三层职责：

| 层 | 职责 | 不做什么 |
|----|------|---------|
| **WsClient** | 维护 WebSocket、自动重连、把出站请求的 `id` 与入站响应的 `req_id` 关联成 Promise、把 `event`/`hello`/`server_closing` 等主动消息派发给 Store | 不持有业务状态 |
| **Store** | 折叠 `AgentEvent` 流为 run→phase→agent 树（逻辑同 TUI 的 `reduce`）、缓存 findings/report/status、记录连接与订阅状态 | 不直接碰 socket |
| **UI** | 纯渲染 + 派发用户意图（提交 run、展开节点、取消、确认脚本） | 不解析协议 |

**数据流**：UI 调用 `wsClient.request(msg)` → 得到 `accepted/ok/error` Promise；同时 `event` 消息经 Store 的 reducer 折叠进 run 树，UI 订阅 Store 选择性重渲染。这与 TUI 的 `tokio::select!{ 客户端消息 / 实时事件 }` 双循环同构，只是搬到浏览器。

---

## 3. 信息架构与屏幕地图

信息层级直接沿用 [TUI 02 §1](tui/02-information-architecture.md)：

```
run
├── phase (1..N)          ← 默认展开
│   └── agent (1..M)      ← 默认折叠，点击展开
│       └── recent_tools  ← 展开时显示工具/消息明细
└── pipeline (0..N)       ← 默认折叠
    └── stage (1..K)
```

### 屏幕地图

```
        ┌──────────────── App Shell ────────────────┐
        │  侧栏: 连接状态 · Run 列表 · "+ 新建 run"  │
        └───────────────────┬────────────────────────┘
                            │
        ┌───────────────────┼───────────────────────────┐
        ▼                   ▼                            ▼
   [A] 新建 run        [B] Live 视图               [C] Replay 详情
   表单 + 脚本预览     实时 phase→agent 树         只读 · 同一棵树
   (run/confirm_run)   (subscribe 实时流)          (get_logs 重建)
                            │                            │
                            └────── Tab 切换 ────────────┘
                       Findings · Report · Raw events
```

| 屏幕 | 入口 | 数据来源 | 交互 |
|------|------|---------|------|
| **[A] 新建 run** | 侧栏 "+ 新建" | `run` / `confirm_run` | 选 NL/workflow/script、填 args、（可选）预览脚本 |
| **[B] Live 视图** | 提交 run 后 / 点击运行中的 run | `subscribe` 实时 `event` 流 | 展开节点、取消 run、筛事件、跟随 |
| **[C] Replay 详情** | 侧栏点击已完成 run | `get_status` + `get_logs` + `get_findings` + `get_report` | 只读浏览，无取消 |

[B] 与 [C] **共用同一棵 run 树组件**——区别只是数据源（实时 `event` vs 一次性 `logs`）和是否显示取消按钮，正如 TUI 的 Live 与 Replay 共用渲染树。

---

## 4. 视觉语言

> 单一事实源：状态映射表与 [TUI 03 §1](tui/03-visual-language.md) **逐行对应**，仅把终端字符换成 Web 等价物（图标字形保留，颜色用 CSS 变量）。

### 4.1 状态图标 + 配色

| 状态 | 图标 | 颜色 token | 来源 |
|------|------|-----------|------|
| Pending | `·` | `--c-dim` (grey) | 尚未 `AgentStarted` |
| Running | `◐` 旋转 | `--c-running` (cyan) | `AgentStarted` 未 `Done` |
| Ok | `✓` | `--c-ok` (green) | `AgentStatus::Ok` |
| Error | `✗` | `--c-error` (red) | `AgentStatus::Error` |
| Cancelled | `⊘` | `--c-dim` (grey) | `AgentStatus::Cancelled` |
| TimedOut | `⌛` | `--c-error` (red) | `AgentStatus::TimedOut` |
| Partial (run 级) | `⚠` | `--c-warn` (yellow) | `RunStatus::Partial` |

**配色规则（同 TUI）**：颜色**永不单独承载语义**，图标符号本身即可区分。提供高对比 / 深浅主题切换；色弱用户可开"仅符号"模式（去色，保留 `✓`/`✗`/`⊘` 形状差异）。

### 4.2 旋转与节奏

- `Running` 的 `◐` 用 CSS `@keyframes spin`（与 TUI 的 `◐◓◑◒` 帧序等价，Web 上用连续旋转）。仅运行中实体旋转，静态状态不动——避免"假装在忙"。
- `agent_progress` 高频事件经节流（见 [§9](#9-实时事件处理)），UI 重绘上限 ~10fps，对齐 TUI 的 10fps 节流，避免闪烁与光敏不适。

### 4.3 数值人性化（同 TUI 03 §3）

集中在 `format.ts` 工具函数，三端共用同一规则：

- **Token**：`>=1M → 4.2M`、`>=1k → 4.2k`，否则原值。展示 `tokens 4.2k↑ 1.1k↓`。
- **耗时**：`<1s → 800ms`、`<1m → 2.1s`、`>=1m → 1m04s`。
- **相对时间**（run 列表）：`2m 前` / `1h 前`。

### 4.4 截断（同 TUI 03 §4）

Web 上用 CSS `text-overflow: ellipsis` + `title` 属性保留全文（hover 可见）：task ~80 字、prompt ~60 字、活动行 ~80 字、工具摘要 ~40 字。代码/报告区域不截断，用滚动容器。

### 4.5 折叠指示

`▾` 展开 / `▸` 折叠，置于 phase / pipeline / 可展开 agent 行首。无子项的 agent 不显三角（与 TUI 一致，避免误导可下钻）。

### 4.6 文案与用词

状态词全屏统一：`running / pending / ok / partial / failed / cancelled / timedout`，与图标一一对应，不混用近义词。按钮动词开头（`取消运行`、`确认执行`、`重新订阅`）。文案集中常量化，中文为基线，便于 i18n。

---

## 5. 核心界面规格

### 5.1 App Shell + Live 视图（[B]）

```
┌────────────────┬──────────────────────────────────────────────┐
│ maestro      ● │  Header: run 0193f2a1…   ⏱ 12.3s   ◐ running  │
│ ws 已连接       │  task: 审查 src/ 的鉴权与输入校验             │
│────────────────│──────────────────────────────────────────────│
│ + 新建 run      │  [ 树 ] [ Findings ] [ Report ] [ Raw ]       │
│                │                                              │
│ 运行中          │  ▾ P0 research (3 planned)  3✓ 0✗            │
│ ◐ 审查 src/… 12s│    ✓ P0#1  gpt-4   read_file·grep   1.2s │
│ ◐ 分析性能 …  8s│    ◐ P0#2  claude  analyzing…       3.4s │
│ 历史            │  ▸ P1 synthesis (2 planned)                  │
│ ✓ 审查 src/… 2m前│                                              │
│ ✗ 重构 错误链…1h前│──────────────────────────────────────────────│
│                │ 输入 4.2k 输出 1.1k · 运行中 3 · 自动滚动 · ✕取消│
└────────────────┴──────────────────────────────────────────────┘
```

| 区域 | 内容 | 数据源 |
|------|------|--------|
| 侧栏顶 | 连接状态徽标（连接中/已连接/重连中/已关闭） | WsClient 状态 |
| 侧栏 "+ 新建" | 打开 [A] 新建 run 表单 | — |
| 侧栏运行中 | 活跃 run 列表（图标 + task 截断 20 字 + 走表耗时） | 本地已知 run + `list_runs` |
| 侧栏历史 | 已完成 run，task 截断 20 字 + 相对时间 | `list_runs` |
| Header L1 | run id + 累计耗时（tick 走表）+ 状态徽标 | run 树根 |
| Header L2 | `task:` 预览（截断 ~80） | `RunStarted` |
| 主区 Tab | 树 / Findings / Report / Raw events | 见下 |
| Footer | 输入 4.2k 输出 1.1k / 运行中 3 / 自动滚动 / 取消 | run 树 + 本地状态 |

> **并发上限不显分母**：footer 只显「运行中 N」（从树里数运行中 agent），**不显 `N/8` 的分母**——并发上限与 quota 是服务端 config，任何 WS 消息（含 `hello`、`StatusOutput`）都不传。若未来 `hello` 扩展 `limits` 字段（见 [§13](#13-已知局限与后续工作)），再补回分母。token 的 `↑↓` 拆分来自实时 `AgentDone.tokens` / `get_logs` 重建，**不来自 `get_status`**（后者只有标量 `total_tokens`，见 [§7](#7-状态模型)）。

### 5.2 新建 run 表单（[A]）

三选一输入模式（对应 `run` payload 的 `nl` / `workflow` / `script`），单选切换：

```
┌────────────────┬──────────────────────────────────────────────┐
│ maestro      ● │  新建 run                                     │
│ ws 已连接       │──────────────────────────────────────────────│
│                │  模式  (●) 自然语言  ( ) Workflow  ( ) 内嵌 Lua│
│ + 新建 run  ◀── │                                              │
│                │  ┌──────────────────────────────────────────┐ │
│ 运行中          │  │ 分析这段代码并找出性能瓶颈                │ │
│ ◐ 0193f2a1 12s │  │                                          │ │
│                │  │                                          │ │
│ 历史            │  └──────────────────────────────────────────┘ │
│ ✓ 0193e9c4 2m前│                                              │
│ ✗ 0193d811 1h前│  args  ┌─ key ──────┬─ value ────────┐  [+]   │
│                │        │ focus      │ hot path       │        │
│                │        └────────────┴────────────────┘        │
│                │                                              │
│                │  ☑ 执行前预览生成的脚本 (confirm)             │
│                │                                              │
│                │                         [ 取消 ]  [ 提交 run ]│
└────────────────┴──────────────────────────────────────────────┘
```

模式切换时中部输入区替换为对应控件：

```
( ) Workflow 文件 →  ┌─ 绝对路径 ───────────────────────────┐
                     │ /home/me/wf/audit.lua                │  服务端本地文件系统
                     └──────────────────────────────────────┘

( ) 内嵌 Lua      →  ┌─ Lua 编辑器（语法高亮）──────────────┐
                     │ 1  local r = agent({ prompt='...' }) │
                     │ 2  ...                               │
                     └──────────────────────────────────────┘
                     2.1 KB / 64 KB   ▓▓▓▓░░░░░░░░░░░░░░░░░░
```

- **64KB 帧上限**：script 模式实时显示字节计数，逼近 64KB 时变红并禁用提交（对齐 [WS §8 消息大小限制](websocket-server.md#8-安全设计)）。
- `confirm` 勾选框仅在 NL 模式可用（workflow/script 无需预览）。
- 校验：`nl`/`workflow`/`script` 恰好一个非空，否则提交按钮禁用并提示——前置拦截 `bad_request`。

### 5.3 脚本预览模态（confirm 流程）

收到 `script_preview` 后弹出模态：

```
┌─ 预览生成的脚本 ─ run 0193f2a1… ──────────────── ⏳ 28s ─┐
│  -- 由 planner 生成                                      │
│  local result = agent({ prompt = '...' })               │  ← 语法高亮，只读
│  ...                                                    │
│──────────────────────────────────────────────────────── │
│                 [ 放弃 ]  [ 延长 30s ]  [ 确认执行 ]      │
└──────────────────────────────────────────────────────────┘
```

- 右上倒计时仅在最后 5 秒显示（`<5s` 时出现 `⏳ 5s` 并开始倒数）；前 25 秒静默不显示倒计时，仅提供「延长 30 秒」按钮供用户主动延长时间。超时自动关闭并提示 `confirm_timeout`。
- "确认执行" → `confirm_run { approve: true }`；"放弃" → `confirm_run { approve: false }`，按 [WS §5.2](websocket-server.md#52-nl-run-脚本预览与确认)。

### 5.4 run 树组件（[B]/[C] 共用）

逐行渲染，列结构对齐 [TUI 02 §3](tui/02-information-architecture.md)：

```
[展开▾] [状态◐] [name]     [model]  [tools 串]        [耗时]  [活动行]
   ▾      ◐     P0#1       opus     read_file·grep    2.1s    投票中…
```

- **name 列显示 `{phase_label}#{序号}`**（如 `P0#1`、`P1#3`）：phase 内 agent 按 `AgentStarted` 到达顺序编号，可读性优于 UUID 前缀。hover 该行用 `title` 显示完整 `prompt_preview`。
- 点击行展开 → 追加 `recent_tools`（容量 5）明细，每条来自 `ProgressDelta`（ToolCall/FileEdit/Message）。
- phase 行折叠时显示 `label · N planned · ok/failed`，展开显示逐 agent。
- pipeline 行折叠显示 `stage i/N · label · items N/M`。
- **活动行**：永远只显示该 agent 最新一条 `ProgressDelta::Message`。
- **依赖 `agent_progress`**：本组件的「tools 串」与「活动行」两列**只来自 `agent_progress` 事件**。因此默认订阅必须包含 `agent_progress`（见 [§9.1](#91-filter服务端侧过滤)），否则这两列恒为空。

### 5.5 Findings Tab

`get_findings`（或实时累积）渲染为可排序表，沿用 `Finding` 结构（`src/core/contract/finding.rs`）：

```
┌──────────────────────────────────────────────────────────────┐
│  [ 树 ] [ Findings ◀ ] [ Report ] [ Raw ]      排序: severity▾ │
│──────────────────────────────────────────────────────────────│
│  sev   kind          title                    location        │
│──────────────────────────────────────────────────────────────│
│  ▾ 🔴 critical  missing_auth   /admin 路由未验证 token  api.rs:42│
│      detail:  GET /admin 可在未登录状态下访问                  │
│      evidence: · 路由表无 auth 中间件                          │
│                · 集成测试 test_admin 未覆盖鉴权                │
│  ▸ 🟠 high      sql_injection  查询拼接用户输入       db.rs:88  │
│  ▸ 🟡 medium    weak_hash      口令用 MD5 存储        auth.rs:15│
│  ▸ ⚪ info       source         参考实现来自 RFC 6749   —        │
└──────────────────────────────────────────────────────────────┘
```

| 列 | 来源 | 渲染 |
|----|------|------|
| severity | `Severity` | 徽标：critical/high=red、medium=yellow、low/info=dim，按 `Ord` 默认降序排 |
| kind | `kind` | 标签 chip |
| title | `title` | 主文本 |
| location | `Location` | `src/api.rs:42`，可点击复制路径 |
| detail / evidence | `detail` + `evidence[]` | 展开行（`▾`）显示详情与证据列表 |

空态：`get_findings` 返回空 → 居中提示「本次 run 无 findings」。

> **运行期间轮询**：run 进行中（status=running），Findings Tab 每 10s 定时调 `get_findings` 刷新。Tab 徽标显示 findings 数量变化（如 `Findings (3)`）提示用户有新结果，而非静默等待 run 结束。

### 5.6 Report Tab

`get_report` 返回 `RunDone.report`（任意 JSON）。约定常见形态 `{ markdown: "..." }` 时渲染 markdown；否则美化 JSON 折叠树。

```
┌──────────────────────────────────────────────────────────────┐
│  [ 树 ] [ Findings ] [ Report ◀ ] [ Raw ]   [复制] [下载 .md ▾]│
│──────────────────────────────────────────────────────────────│
│  # 分析报告                                                    │
│                                                              │
│  ## 摘要                                                      │
│  共发现 4 处问题，其中 1 处 critical…                         │
│                                                              │
│  ## 性能瓶颈                                                  │
│  - `parse_loop()` 在热路径上重复分配（src/main.rs:120）       │
│  - …                                                         │
│                                                              │
│  ▌（markdown 渲染区，长内容滚动）                              │
└──────────────────────────────────────────────────────────────┘
```

report 非 `{ markdown }` 形态时退化为可折叠 JSON 树，下载按钮切换为 `.json`。run 未结束（无 `RunDone.report`）时显「报告将在 run 完成后生成」。

### 5.7 Raw events Tab

原始 `AgentEvent` 流（实时来自 `event`，回看来自 `get_logs`），虚拟滚动表格 + 类型筛选器（多选 chips，对应 [§9](#9-实时事件处理) 的 filter）。供调试与"看到底发生了什么"。

```
┌──────────────────────────────────────────────────────────────┐
│  [ 树 ] [ Findings ] [ Report ] [ Raw ◀ ]    □详细模式(含progress)│
│  筛选: [✓run_*] [✓phase_*] [✓agent_started] [✓agent_done] [□progress]│
│──────────────────────────────────────────────────────────────│
│  ts        type            摘要                                │
│──────────────────────────────────────────────────────────────│
│  00:00.0   run_started     task: 审查 src/ 的鉴权…             │
│  00:00.2   phase_started   P0 research · planned 3             │
│  00:00.3   agent_started   P0#1 · gpt-4 · "搜索 verify…"   │
│  00:01.5   agent_done      P0#1 · ok · 1.2k↑ · 1.2s        │
│  00:03.4   agent_done      P0#2 · ok · 800↑ · 3.4s         │
│  00:03.5   phase_done      P0 · ok 3 · failed 0                │
│  ▌ … 虚拟滚动，自动跟随底部（follow 开时）                      │
└──────────────────────────────────────────────────────────────┘
```

「详细模式」勾选 = 以 `filter:null` 重订阅当前 run（含高频 `agent_progress`），对齐 [§9.1](#91-filter服务端侧过滤)。

### 5.8 Replay 详情（[C]）

点击侧栏已完成 run 进入。**复用 [B] 的全部布局与 Tab**，差异仅三处：状态徽标为终态（不旋转）、无取消按钮、无 follow（树一次性重建，不走表）。

```
┌────────────────┬──────────────────────────────────────────────┐
│ maestro      ● │  Header: run 0193e9c4…   ⏱ 2m04s   ✓ completed│
│ ws 已连接       │  task: 审查 src/ 的鉴权与输入校验             │
│────────────────│──────────────────────────────────────────────│
│ + 新建 run      │  [ 树 ] [ Findings ] [ Report ] [ Raw ]       │
│                │                                              │
│ 运行中          │  ▾ P0 research (3 planned)  3✓ 0✗            │
│ ◐ 0193f2a1 12s │    ✓ P0#1  gpt-4   read_file·grep   1.2s │
│                │    ✓ P0#2  claude  分析完成          3.4s │
│ 历史            │  ▾ P1 synthesis (2 planned)  2✓ 0✗          │
│ ✓ 0193e9c4 ◀── │    ✓ P1#1  opus    汇总报告          5.1s │
│ ✗ 0193d811 1h前│                                              │
│                │──────────────────────────────────────────────│
│                │ tokens 8.4k↑ 3.1k↓ · 只读回放 · ⟳ 刷新        │
└────────────────┴──────────────────────────────────────────────┘
```

数据来源（并发拉取，见 [§8.4](#84-回看历史-run-c-replay)）：Header 走 `get_status`、树走 `get_logs` 重建、Findings/Report 走各自查询。`✗ failed` 的 run 同样可回放，Report Tab 显示失败原因。

### 5.9 连接态与空态

**空态**（无任何 run，首次进入）：

```
┌────────────────┬──────────────────────────────────────────────┐
│ maestro      ● │                                              │
│ ws 已连接       │            还没有任何 run                     │
│                │                                              │
│ + 新建 run      │         提交第一个 run 开始观察               │
│ (无 run)        │                                              │
│                │              [ + 新建 run ]                   │
└────────────────┴──────────────────────────────────────────────┘
```

**连接异常**（重连中 / 服务端关闭）：顶部 sticky 条（不推动内容，覆盖在内容上方）+ 侧栏徽标变色，运行树保留为只读快照（不丢失已折叠状态）：
```
╔══════════════════════════════════════════════════════════╗
║ ⚠ 重连中… 第 2 次（4s 后重试）     [ 立即重连 ]          ║  ← 黄色条，position:sticky
╠══════════════════════════════════════════════════════════╣
│ maestro      ◐ │  （主区内容保留，但所有提交/取消按钮禁用）    │
│ ws 重连中       │                                             │

收�� server_closing → 条变红「服务端已关闭」，按钮文案变 [ 重新连接 ]。
```

各状态徽标颜色见 [§10 连接级状态表](#10-错误与边界状态)。

---

## 6. WebSocket 客户端层

`WsClient` 封装 [WS §3 协议](websocket-server.md#3-消息协议) 的全部往返语义。

### 6.1 请求/响应关联

每个出站 `ClientMsg` 携带自增 `id`（如 `req-{n}`）。`WsClient.request(type, payload)` 返回 Promise，在收到 `req_id === id` 的 `accepted/ok/error/status/run_list/logs/findings/report/script_preview` 时 resolve/reject：

```ts
const { run_id } = await ws.request("run", { nl, confirm: false }); // → accepted
await ws.request("subscribe", { run_id, filter });                 // → ok
```

无 `req_id` 的主动消息（`event`/`hello`/`server_closing`）不走 Promise，直接 emit 给 Store。

### 6.2 能力协商

连接后第一条必是 `hello`。`WsClient` 缓存 `capabilities`，UI 据此灰显未支持的功能（向前兼容，不硬编码版本判断）。

### 6.3 重连与心跳

- 应用级心跳：每 20s 发 `ping`，超时未收 `pong` 判定连接异常。
- 断线指数退避重连（1s→2s→4s…上限 30s）。重连成功后，对每个"本地认为仍在运行"的 run 自动重发 `subscribe`，并按 [§9.3](#93-断线补齐) 用 `get_logs` 补齐空窗期事件。
- 收到 `server_closing` → 停止重连，UI 显示"服务端已关闭"，所有 run 树进入只读。

### 6.4 请求超时

每个 `request` 设 15s 客户端超时（脚本预览的 `confirm_timeout` 由服务端 30s 把关，不在此列），超时 reject 并提示，不影响连接。

---

## 7. 状态模型

Store 顶层（TypeScript 形态，逻辑对应 TUI 的 `AppState`）：

```ts
interface AppState {
  conn: "connecting" | "open" | "reconnecting" | "closing" | "closed";
  capabilities: string[];
  runs: Record<RunId, RunState>;     // 所有已知 run（实时 + 历史）
  selectedRun: RunId | null;
  activeTab: "tree" | "findings" | "report" | "raw";
  pendingConfirm: { runId: RunId; script: string; deadline: number } | null;
}

interface RunState {
  runId: RunId;
  task: string;
  status: "running" | RunStatus;     // running 或 RunDone.status
  phases: PhaseNode[];               // reduce(AgentEvent) 折叠结果
  pipelines: PipelineNode[];
  findings: Finding[];
  report: unknown | null;
  tokens: TokenUsage;
  startedAt: number;
  updatedAt: number;
  subscribed: boolean;
  filter: string[] | null;
  follow: boolean;                   // 是否自动滚到最新活动
  expanded: Set<string>;             // 展开的节点 key（phase/agent）
  lagged: boolean;                   // 收到过 broadcast lagged 警告
  rawEvents: AgentEvent[];           // Raw tab，容量受限/虚拟滚动
}
```

### reduce(event)：事件折叠（与 TUI 同构）

| 事件 | 折叠动作 |
|------|---------|
| `RunStarted` | 建/更新 `RunState`，置 `task`、`startedAt` |
| `PhaseStarted` | push `PhaseNode { id, label, planned }`，默认展开 |
| `AgentStarted` | 在对应 phase 下建 `AgentNode`（status=running），默认折叠 |
| `AgentProgress` | 按 `ProgressDelta` 更新该 agent 的活动行 / 工具串（容量 5 环形）/ token |
| `AgentDone` | 置 agent status/tokens/elapsed，停旋转 |
| `PhaseDone` | 置 phase 的 ok/failed 汇总 |
| `RunDone` | 置 run status/total_tokens/report，停 Header 走表 |
| `Log` | 追加日志（Raw tab + 可选 toast，warn/error 级别） |
| `Pipeline*` | 维护 `PipelineNode` 与 stage/item 进度 |

reducer 是纯函数，便于单测——直接喂 [TUI 14 mock-scenarios](tui/14-mock-scenarios.md) 的事件序列即可验证三端折叠一致。

---

## 8. 关键用户流程

### 8.1 提交并实时观看（标准流程）

```
用户填 NL 表单 → 点提交
  → ws.request("run", { nl, confirm:false }) → accepted { run_id }
  → Store 建 RunState，路由到 [B] Live 视图
  → ws.request("subscribe", { run_id, filter:null }) → ok   // 全量，含 agent_progress
  → 持续收到 event → reduce → 树实时推进
  → run_done → Header 徽标转终态，自动拉 get_report / get_findings 填充 Tab
```

默认订阅**全量事件**（`filter:null`，含 `agent_progress`），因为树的 tools 串/活动行依赖它（见 [§9.1](#91-filter服务端侧过滤)）；流量靠客户端 ~10fps 节流消化（[§9.2](#92-客户端节流)）。仅在弱网或多 run 并行时才允许用户手动切到「里程碑模式」（排除 `agent_progress`，代价是 tools/活动行变空）。

### 8.2 NL 预览确认

```
用户勾选"执行前预览" → 提交 → accepted → script_preview { script }
  → 弹预览模态（语法高亮 + 30s 倒计时）
  → 用户"确认执行" → confirm_run { approve:true } → ok → 进入 §8.1 实时流
     或"放弃" → confirm_run { approve:false } → ok → 关闭模态，run 被清理
     或超时 → error(confirm_timeout) → 提示并关闭
```

### 8.3 取消运行

```
用户点 ✕取消 → 确认弹窗 → cancel { run_id } → ok（仅表示信号已发）
  → UI Header 徽标转"cancelling…"（旋转）
  → 异步收到 event { run_done { status:"cancelled" } } → 转终态 ⊘
```

对齐 [WS §5.3 异步性说明](websocket-server.md#53-取消-run)：`ok` 不代表已停，等 `run_done` 才转终态。

### 8.4 回看历史 run（[C] Replay）

```
侧栏点击已完成 run
  → 并发 get_status + get_logs(offset:0) + get_findings + get_report
  → logs.items 喂同一 reduce 重建树（一次性，无旋转）
  → Findings/Report Tab 直接填充
  → 树为只读：无取消按钮、无 follow
```

> **token 拆分从 logs 重建，不用 status**：`get_status` 的 `StatusOutput.total_tokens` 只是标量总数（[cli.rs:89](../../src/cli.rs#L89)），拿不到 `↑↓` 拆分。Replay 的 token 列由 `get_logs` 里的 `AgentDone.tokens`（`TokenUsage`）累加得出。`get_status` 仅用于秒出 Header 概览（task/status/耗时），细节等 logs 到位再补全。

### 8.5 恢复未完成 run

```
侧栏对\"疑似中断\"的 run **在其详情 Header 内**显\"恢复\"按钮（不在侧栏条目上直接展示，降低误触）：

```
┌─ Header: run 0193f2a1…   ⏱ —   ◐ running?  [ 恢复运行 ] ──┐
│  task: 审查 src/ 的鉴权…                                  │
```
  → resume { run_id } → accepted（或 error: run_finished / not_found / no workflow）
  → 成功后等价 §8.1，自动 subscribe 接续实时流
```

> **"中断"只能启发式推断**：checkpoint 没有显式的 `interrupted` 状态——崩溃的 run 其 `status` 可能仍停在 `running`。UI 判据：`status` ∉ {completed, failed, cancelled, partial} **且** 不在本地活跃订阅集里 → 视为疑似中断，显恢复按钮。判错也无害：`resume` 对终态 run 会回 `error(run_finished)`，UI 据此把它降级为只读 Replay。

---

## 9. 实时事件处理

### 9.1 filter：服务端侧过滤

`subscribe` 的 `filter` 在服务端就丢弃未列出的事件类型（[WS §4.4 FilteredStream](websocket-server.md#44-handlerrs)），从源头减流量。

**默认全量（`filter: null`，含 `agent_progress`）**。这是有意的取舍：树的 tools 串/活动行/实时 token 走表全部依赖 `agent_progress`（见 [§5.4](#54-run-树组件b c-共用)），排除它会让 Live 视图退化成只有里程碑的骨架。高频流量交给客户端节流（[§9.2](#92-客户端节流)）消化,而非靠服务端过滤。

**自动降级**：客户端监测连接质量（连续 N 次 broadcast lagged 警告或 WS 缓冲区积压），自动从全量事件切到「里程碑模式」（排除 `agent_progress`），代价是 tools 串/活动行变空、token 只在 `agent_done` 时跳变。降级时 Header 显黄条 `⚠ 已切到节能模式`，并提供「恢复全量」按钮供用户手动切回。切换 = `unsubscribe` 后用新 filter 重新 `subscribe`。

`submit_run` 的 `filter` 在服务端就丢弃未列出的事件类型（[WS §4.4 FilteredStream](websocket-server.md#44-handlerrs)），从源头减流量。

### 9.2 客户端节流

即便收 `agent_progress`，UI 重绘节流到 ~10fps：reducer 立即更新内存状态，渲染用 `requestAnimationFrame` 合批。活动行/走表/token 计数都走这条节流路径，避免每个 token delta 触发 React 重渲染。

### 9.3 断线补齐

重连后空窗期事件已从 broadcast 丢失。补齐策略（对齐 [WS §5.5](websocket-server.md#55-断线重连)）：

- **run 仍运行**：重发 `subscribe`（从重连时刻起的新事件）+ `get_logs { offset: 已折叠条数 }` 拉空窗事件，按事件去重后补进树。
- **run 已完成**：`subscribe` 回 `error(run_finished)` → 转走 `get_logs` + `get_report` + `get_findings` 重建只读详情。

### 9.4 broadcast lagged

收到注入的 `log` 警告（`[ws] broadcast lagged: N events dropped`）时（[WS §7 广播滞后](websocket-server.md#7-错误处理)）：Store 置 `run.lagged = true`，Header 显黄色 `⚠ 事件有丢失` 徽标，并提供"用 get_logs 补齐"按钮，一键 `get_logs` 重对齐。

---

## 10. 错误与边界状态

UI 对 [WS §7 错误码](websocket-server.md#7-错误处理) 逐一有反馈：

| `code` | UI 反馈 |
|--------|---------|
| `bad_request` | 表单内联校验错误（理想情况下提交前已拦截） |
| `not_found` | toast "run 不存在或已清理"，从侧栏移除该条 |
| `run_finished` | subscribe 自动降级为 get_logs 重建（§9.3），不报错给用户 |
| `already_running` | resume 按钮提示"已在运行"，改为直接 subscribe |
| `backend_error` | 醒目错误条 "后端无法启动（如 opencode 未安装）" |
| `capacity` | toast "已达并发上限（max_runs），稍后重试"，提交按钮短暂禁用 |
| `confirm_timeout` | 预览模态关闭 + 提示"预览超时，请重新提交" |
| `internal` | toast "服务端内部错误" + 折叠的原始 message 供排查 |

连接级状态：

| 连接状态 | UI |
|---------|-----|
| connecting | 侧栏徽标转圈，主区骨架屏 |
| open | 绿点 `●` 已连接 |
| reconnecting | 黄点，顶部条"重连中… 第 N 次"，运行树保留只读 |
| closed / server_closing | 红点，"连接已关闭"，禁用所有提交，提供"重新连接"按钮 |

**空状态**：无任何 run 时主区显引导卡片"提交第一个 run"，直达 [A] 表单。

---

## 11. 技术选型与依赖

| 关注点 | 选型 | 理由 |
|--------|------|------|
| 框架 | React + TypeScript | 生态成熟、reducer 模型契合事件折叠 |
| 构建 | Vite | 快、零配置、本地 dev server |
| 状态 | Zustand（或 Redux Toolkit） | 轻量、外部可订阅、便于把 reducer 单测 |
| 样式 | CSS 变量 + Tailwind（或 CSS Modules） | 主题/无障碍配色用 CSS 变量集中管理 |
| 代码高亮 | Shiki / CodeMirror | Lua 脚本预览与 Report 渲染 |
| Markdown | markdown-it + 安全过滤 | Report 的 `{ markdown }` 渲染，转义防 XSS |
| WebSocket | 原生 `WebSocket` + 自写 `WsClient` | 协议简单，无需额外库 |

**部署形态**：纯静态 SPA。两种交付：

1. **独立 dev**：`vite dev`，连本机 `ws://127.0.0.1:7474/ws`（CORS 已在服务端允许 `localhost`/`127.0.0.1`，见 [WS §8](websocket-server.md#8-安全设计)）。
2. **内嵌（后续）**：构建产物可由 `maestro serve` 用 `tower-http::services::ServeDir` 在 `/` 托管，做到"一个二进制带 UI"——记入 [§13](#13-已知局限与后续工作)，v0.1 先独立交付。

源码目录建议 `web/`（与 Rust `src/` 平级，独立 `package.json`，不污染 Cargo workspace）。

---

## 12. 实现计划

| 阶段 | 内容 | 产物 | 估时 |
|------|------|------|------|
| **W1** | 脚手架 + `WsClient`（连接/hello/ping-pong/req-id 关联/重连） | `web/` + `wsClient.ts` | 2 h |
| **W2** | Store + `reduce(AgentEvent)`（纯函数，单测喂 mock 事件序列） | `store.ts` + 单测 | 2.5 h |
| **W3** | App Shell + 侧栏（连接徽标 + `list_runs`） | 布局组件 | 1.5 h |
| **W4** | run 树组件（phase→agent→tools，状态图标/旋转/折叠，[B]/[C] 共用） | `RunTree.tsx` | 3 h |
| **W5** | 新建 run 表单 + 脚本预览模态（run / confirm_run / 倒计时） | `NewRun.tsx` | 2.5 h |
| **W6** | Findings / Report / Raw Tab | 3 个 Tab 组件 | 2 h |
| **W7** | 实时订阅 + filter 切换 + 节流 + 断线补齐 + lagged 处理 | 接入 §9 | 2.5 h |
| **W8** | 错误/边界状态全覆盖 + 空状态 + 无障碍主题 | §10 | 1.5 h |

总计约 **17.5 h**。W1–W2 可独立先行（用 mock WS server 或录制的事件 JSONL 驱动 reducer 单测），无需后端就绪。

### 测试策略

- **reducer 单测**：复用 [TUI 14 mock-scenarios](tui/14-mock-scenarios.md) 的 8 个事件场景，断言折叠后的树形与 TUI 一致（保证三端语义对齐）。
- **WsClient 单测**：用 `mock-socket` 模拟服务端，验证 req-id 关联、重连、超时、ping/pong。
- **端到端**：起真实 `maestro serve`（MockBackend），Playwright 跑 §8 的五条流程。

---

## 13. 已知局限与后续工作

> 下列前 4 条是 **UI 想要、但当前协议/契约不提供** 的数据缺口（审计自 [event.rs](../../src/core/contract/event.rs) 与 [cli.rs StatusOutput](../../src/cli.rs#L81)）。UI v0.1 均已降级处理，标注「↗ 需协议扩展」者为推荐的后续增强。

- **agent 可读名缺失** ↗：[`AgentStarted`](../../src/core/contract/event.rs) 无别名字段，无法显示 Lua 脚本中给 agent 起的别名（如 `adversary#1`）。**v0.1 降级**：name 列用 `{phase_label}#{序号}`（如 `P0#1`），hover 显 `prompt_preview`。**增强**：给 `AgentStarted` 加 `alias: Option<String>`。
- **并发上限 / quota 不传** ↗：footer 只显「运行中 N」，无分母。并发上限与 quota 是服务端 config，`hello`/`StatusOutput` 都不含。**增强**：`hello` 加 `limits: { max_runs, max_concurrency, quota }`，footer 再补回 `N/8`、`quota 5/1000`。
- **token 拆分仅实时可得**：`get_status.total_tokens` 是标量（[cli.rs:89](../../src/cli.rs#L89)），`↑↓` 拆分只能从实时 `AgentDone.tokens` 或 `get_logs` 重建。**降级**：Replay 用 logs 重建，纯 status 快照只显总数。
- **"中断"状态需启发式**：checkpoint 无 `interrupted` 状态，UI 靠「非终态 + 不在活跃集」推断是否可 resume（见 [§8.5](#85-恢复未完成-run)）。判错由 `resume` 回 `error(run_finished)` 兜底。**增强**：checkpoint/StatusOutput 显式区分 `interrupted` 与 `running`。
- **事件回放重叠去重**：`get_logs` 补齐与实时 `subscribe` 流存在短暂重叠（[WS §11](websocket-server.md#11-已知局限与后续工作)），客户端需按 (run_id, 序号/时间) 去重——当前 reducer 用幂等更新缓解，但缺乏服务端单调序号。建议后续给 `AgentEvent` 加全局 seq 字段。
- **report 形态约定**：`RunDone.report` 是任意 JSON，UI 只对 `{ markdown }` 有专门渲染，其余降级为 JSON 树。需要与 planner 约定稳定的 report schema。
- **findings 实时性**：findings 通过 `get_findings` 一次性拉取；run 进行中若想实时累积 findings，依赖事件流里是否含 finding 事件（当前 `AgentEvent` 无 `FindingReported` 变体）——记为对 WS 协议的潜在扩展。
- **多 run 并发 UI**：协议支持单连接订阅多 run，但 v0.1 UI 同一时刻只详看一个 run（侧栏切换）。并排对比多 run 留待后续。
- **内嵌托管**：v0.1 独立 SPA；后续由 `maestro serve` 用 `ServeDir` 托管静态产物，实现单二进制自带 UI。
- **鉴权 UI**：服务端 v0.1 无鉴权（依赖绑定 localhost）。未来服务端加 `Bearer token` 后，UI 需加 token 输入与持久化。
- **移动端 / 响应式**：当前为桌面优先布局，窄屏折叠侧栏的响应式留待后续。

---

## 相关文档

- WebSocket 服务器：[websocket-server.md](websocket-server.md)（消息协议、错误码、连接生命周期）
- TUI 信息架构：[tui/02-information-architecture.md](tui/02-information-architecture.md)（信息层级、屏幕地图）
- TUI 视觉语言：[tui/03-visual-language.md](tui/03-visual-language.md)（状态图标/配色单一事实源）
- TUI 状态模型：[tui/07-state-model.md](tui/07-state-model.md)（`AppState` + `reduce`，Web reducer 的同构参照）
- 事件契约：[`src/core/contract/event.rs`](../../src/core/contract/event.rs)（`AgentEvent` 完整定义）
- Finding 契约：[`src/core/contract/finding.rs`](../../src/core/contract/finding.rs)（`Finding` 结构）
- 架构总览：[architecture.md](../architecture.md)
- WS 协议测试：[ws-test.md](ws-test.md)（`maestro test-ws` 命令，按场景验证协议）
