# Maestro Web 重设计方案

> **状态:** Draft  
> **日期:** 2025-08-19  
> **范围:** `web/` 目录完整产品级重设计

---

## 1. 产品定位

**一句话:** AI 智能体编排的控制中心——从设计、执行、观察到迭代的完整工作台。

**目标用户:** 使用多 Agent 构建复杂任务的开发者与技术团队。

**核心问题:** 当前 Maestro 是一个监控面板，用户只能查看运行状态和日志。重设计后，用户可以在浏览器中完成「设计工作流 → 执行 → 实时观察 → 分析结果 → 迭代优化」的完整闭环，无需切换到终端。

**设计原则:**

| 原则 | 说明 |
|------|------|
| 编排优先 | 可视化工作流编辑器是第一公民，不是附属功能 |
| 降低门槛 | 自然语言输入即可启动任务，不必先写 Lua |
| 信息密度 | 专业工具需要高信息密度，但通过层次感降低认知负荷 |
| 实时反馈 | 运行状态、Agent 输出、事件流全部实时推送 |
| 键盘驱动 | Cmd+K 全局搜索、快捷键操作，面向高频用户 |

---

## 2. 信息架构

### 2.1 导航结构变更

**当前:** 顶部导航栏 + 5 个扁平页面

```
TopNav: [Dashboard] [Runs] [Workflows] [Backends] [Run]
```

**重设计:** 可折叠侧边栏 + 三层页面结构

```
Sidebar (56px collapsed / 240px expanded)
├── 首页 (/)
├── ─────────
├── 工作流 (/workflows/:id)
├── 运行历史 (/runs/:id)
├── 后端 (/backends)
├── ─────────
└── 设置 (/settings)
```

**变更理由:**

- 侧边栏为工作流编辑器释放水平空间（编辑器需要全宽画布）
- 工作流列表和运行历史从独立页面降级为侧边栏内嵌面板，减少导航跳转
- 顶部导航栏的空间留给页面级操作（运行、保存、分享）

### 2.2 路由表

| 路由 | 页面 | 说明 |
|------|------|------|
| `/` | 工作区首页 | 快速启动 + 最近活动 + 统计概览 |
| `/workflows/:id` | 工作流编辑器 | 可视化编排 + 代码编辑（核心页面） |
| `/runs/:id` | 运行详情 | 实时监控 + 结果分析 |
| `/backends` | 后端管理 | Provider 配置（保持不变） |
| `/settings` | 设置 | 偏好、API 密钥、通知规则（新增） |

**移除的路由:**

- `/runs` → 运行列表改为首页的「最近运行」面板 + 全局搜索
- `/workflows` → 工作流列表改为侧边栏内嵌面板

---

## 3. 视觉设计

### 3.1 设计语言

**关键词:** 克制 · 深邃 · 精准 · 低噪音

定位为「精密仪器」而非通用管理面板。长时间使用不疲劳，信息一目了然。

### 3.2 色彩体系

```css
/* 背景层 — 比当前更深，降低视觉噪音 */
--color-bg-base:     #080c12;   /* was #0a0e14 */
--color-bg-surface:  #11161e;   /* was #141821 */
--color-bg-elevated: #181d28;   /* was #1c2230 */
--color-bg-overlay:  #1e2432;   /* 新增：模态/弹层 */

/* 主色 — teal 替代亮绿，更沉稳 */
--color-primary:     #14b8a6;   /* was #00e676 */
--color-accent:      #6366f1;   /* 新增：indigo，用于高亮/强调 */

/* 语义色 */
--color-success:     #10b981;
--color-running:     #3b82f6;
--color-failed:      #ef4444;
--color-warning:     #f59e0b;
--color-pending:     #52525b;

/* 文字 */
--color-text-primary:   #eceff4;
--color-text-secondary: #9ca3af;
--color-text-muted:     #555b66;

/* 边框 */
--color-border:        #1e2432;
--color-border-strong: #2a3142;
```

**换掉 `#00e676` 的理由:** 亮绿在长时间使用的专业工具中过于刺眼。Teal（`#14b8a6`）更冷静，饱和度适中，适合开发者长时间盯屏。

### 3.3 字体系统

```css
--font-display: 'Cal Sans', system-ui, sans-serif;    /* 标题、品牌 */
--font-sans:    'Inter', system-ui, sans-serif;         /* 正文 */
--font-mono:    'JetBrains Mono', monospace;            /* 代码、数据 */
```

**换掉 Space Grotesk 的理由:** Cal Sans 更有性格且开源，适合作为产品标识字体。Space Grotesk 过于常见。

### 3.4 视觉特征

| 特征 | 实现 |
|------|------|
| 网格背景纹理 | base 背景叠加 `rgba(255,255,255,0.015)` 的 1px 网格，营造工作台质感 |
| 玻璃态面板 | 卡片使用 `backdrop-blur(12px)` + `bg-surface/70` + 半透明边框 |
| 克制的动画 | 仅保留 3 个：hover 微升 (`translateY(-1px)`)、运行脉冲、页面进入淡入 |
| 节点连线 | 工作流编排中 Agent 节点之间的连线是视觉核心，使用贝塞尔曲线 + 流动动画 |

### 3.5 圆角与间距

```css
--radius-sm: 6px;    /* was 4px — 稍大，更柔和 */
--radius-md: 10px;   /* was 8px */
--radius-lg: 14px;   /* was 12px */
```

间距系统沿用 Tailwind 默认 4px 基准。

---

## 4. 页面设计

### 4.1 工作区首页 (`/`)

**目标:** 用户打开 Maestro 后 3 秒内知道「最近发生了什么」和「下一步做什么」。

**布局:**

```
┌──────────────────────────────────────────────────────┐
│                                                      │
│  ┌─ 快速启动 ────────────┐  ┌─ 今日统计 ──────────┐  │
│  │                       │  │                      │  │
│  │  描述你的任务...       │  │  12 运行  89% 成功   │  │
│  │  ┌──────────────────┐ │  │  1.2k tokens         │  │
│  │  │ 自然语言输入框    │ │  │  4 活跃              │  │
│  │  └──────────────────┘ │  │                      │  │
│  │  选择工作流: [▼]      │  │                      │  │
│  │  选择后端:   [▼]      │  │                      │  │
│  │  [开始运行 →]         │  │                      │  │
│  └───────────────────────┘  └──────────────────────┘  │
│                                                      │
│  ┌─ 最近运行 ──────────────────────────────────────┐ │
│  │  ◉ 代码审查 · 刚才      ● 运行中  阶段 2/4      │ │
│  │  ○ 安全审计 · 3分钟前   ✓ 完成   1.2k tokens    │ │
│  │  ○ 文档生成 · 12分钟前  ✗ 失败                  │ │
│  │  查看全部 →                                     │ │
│  └────────────────────────────────────────────────┘ │
│                                                      │
│  ┌─ 工作流模板 ────────────────────────────────────┐ │
│  │  [代码审查] [安全审计] [文档生成] [+ 自定义]      │ │
│  └────────────────────────────────────────────────┘ │
│                                                      │
└──────────────────────────────────────────────────────┘
```

**组件:**

- `QuickStart` — 自然语言输入框 + 工作流/后端选择器 + 运行按钮。对应 CLI 的 `luft run "自然语言"`。
- `StatsOverview` — 当日统计卡片组（运行数、成功率、token 消耗、活跃数）
- `RecentRuns` — 最近 5 条运行，点击跳转详情
- `WorkflowTemplates` — 快速启动常用工作流

**数据源:**

- 统计: `GET /api/stats` (React Query, 30s stale)
- 最近运行: `GET /api/runs?limit=5&sort=recent`
- 模板: `GET /api/workflows?filter=template`

### 4.2 工作流编辑器 (`/workflows/:id`) — 核心页面

**目标:** 让用户用拖拽方式设计多 Agent 工作流，同时保持代码可编辑性。

**布局:**

```
┌──────────────────────────────────────────────────────┐
│  ← 返回   审计工作流   [可视化 | 代码]  [保存] [运行▶]│
│                                                      │
│  ┌── 节点面板 ──┐  ┌── 画布 / 代码编辑器 ────────┐  │
│  │              │  │                                │  │
│  │  ┌────────┐  │  │   ┌─────────┐    ┌─────────┐  │  │
│  │  │ Producer│  │  │   │ 审查者   │───▶│ 对抗者   │  │  │
│  │  └────────┘  │  │   │producer │    │adversary│  │  │
│  │  ┌────────┐  │  │   └─────────┘    └────┬────┘  │  │
│  │  │Adversary│  │  │                     │       │  │
│  │  └────────┘  │  │                ┌─────▼─────┐ │  │
│  │  ┌────────┐  │  │                │  投票者    │ │  │
│  │  │  Voter  │  │  │                │  voter    │ │  │
│  │  └────────┘  │  │                └───────────┘ │  │
│  │  ┌────────┐  │  │                                │  │
│  │  │ Phase   │  │  │                                │  │
│  │  └────────┘  │  │                                │  │
│  │              │  │                                │  │
│  └──────────────┘  └────────────────────────────────┘  │
│                                                      │
│  ┌── 节点属性 ──────────────────────────────────────┐ │
│  │  名称: 审查者    角色: Producer                   │ │
│  │  模型: claude-sonnet-4    预算: 2000 tokens      │ │
│  │  提示词: [编辑...]                                │ │
│  └──────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────┘
```

**双模式:**

1. **可视化模式** — React Flow 画布
   - 左侧节点面板：拖拽 Producer / Adversary / Voter / Phase 等节点到画布
   - 节点间拖拽连线，定义执行顺序和数据流向
   - 点击节点 → 底部属性面板编辑配置
   - 选中连线 → 可删除或配置条件

2. **代码模式** — CodeMirror Lua 编辑器
   - 同步显示当前可视化对应的 Lua 代码
   - 直接编辑代码，切回可视化时自动解析更新

**双向同步规则:**

- 可视化 → 代码：每次画布变更（增删节点/边、修改属性）生成对应 Lua 代码
- 代码 → 可视化：切到可视化模式时解析 Lua 代码重建画布
- 冲突处理：代码模式的手动修改优先，可视化模式只覆盖结构化字段

**新增依赖:**

- `reactflow` (或 `@xyflow/react`) — 可视化画布
- `zustand` — 编辑器状态管理（已有）

**编辑器状态 (Zustand store):**

```typescript
interface WorkflowEditorState {
  nodes: Node[]
  edges: Edge[]
  selectedNodeId: string | null
  dirty: boolean                    // 是否有未保存修改
  mode: 'visual' | 'code'
  luaCode: string                   // 当前 Lua 源码

  // actions
  addNode: (type: NodeType, position: XYPosition) => void
  removeNode: (id: string) => void
  updateNode: (id: string, data: Partial<NodeData>) => void
  addEdge: (source: string, target: string) => void
  removeEdge: (id: string) => void
  selectNode: (id: string | null) => void
  setMode: (mode: 'visual' | 'code') => void
  syncFromCode: (lua: string) => void
  syncToCode: () => string
  markClean: () => void
}
```

### 4.3 运行详情 (`/runs/:id`)

**目标:** 实时展示运行进度，快速定位问题和查看 Agent 输出。

**布局:**

```
┌──────────────────────────────────────────────────────┐
│  ← 返回   运行 #a3f7c   代码审查   ● 运行中          │
│                                                      │
│  ┌─ 概览栏 ────────────────────────────────────────┐ │
│  │  ████████████░░░░░░  阶段 3/5 · 安全审查中       │ │
│  │  耗时 2m34s · 1.2k tokens · 4 agents            │ │
│  └──────────────────────────────────────────────────┘ │
│                                                      │
│  ┌─ 阶段时间线（横向）──────────────────────────── ┐ │
│  │  ●────●──────◉──────○──────○                    │ │
│  │  初始化  分析   审查   汇总   报告               │ │
│  │  0.3s   12s    运行中  —     —                  │ │
│  └──────────────────────────────────────────────────┘ │
│                                                      │
│  ┌─ Agent 网格 ────────────────────────────────────┐ │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐      │ │
│  │  │ 审查者    │  │ 对抗者    │  │ 投票者    │      │ │
│  │  │ producer  │  │ adversary│  │ voter    │      │ │
│  │  │ 320 tok  │  │ 180 tok  │  │ 等待中    │      │ │
│  │  │ 8 calls  │  │ 3 calls  │  │          │      │ │
│  │  └──────────┘  └──────────┘  └──────────┘      │ │
│  └──────────────────────────────────────────────────┘ │
│                                                      │
│  ┌─ 事件流 ──────────────────────── [展开 ▼] ──────┐ │
│  │  14:32:01  producer   tool_call   read_file      │ │
│  │  14:32:03  producer   tool_result 200 OK         │ │
│  │  14:32:05  adversary  tool_call   grep           │ │
│  └──────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────┘
```

**改进点:**

| 当前 | 重设计 |
|------|--------|
| 左侧固定 timeline sidebar (w-52) | 横向时间线，释放垂直空间 |
| 事件流始终占据底部大块区域 | 默认折叠，点击展开 |
| Agent 卡片在 phase 卡片内嵌套 | 独立 Agent 网格，扁平化 |
| 模拟的进度更新 (setInterval) | WebSocket 实时推送 |
| DetailDrawer 只有 Overview/Prompt/Output | 增加 Findings tab，展示该 Agent 发现的问题 |

**Agent 卡片交互:**

- 点击卡片 → 右侧 Sheet 展开 DetailDrawer（保留现有设计）
- 卡片显示: 角色徽章、状态、token/calls/elapsed、输出预览（前 3 行）
- 运行中的卡片有脉冲边框动画

### 4.4 全局搜索 / 命令面板 (Cmd+K)

**触发:** `Cmd+K` (macOS) / `Ctrl+K` (Windows)

**功能:**

```
┌──────────────────────────────────────┐
│  🔍 搜索或输入命令...                │
│ ─────────────────────────────────── │
│  最近运行                            │
│  → 代码审查 #a3f7c    运行中         │
│  → 安全审计 #b8e2d    完成           │
│ │                                    │
│  工作流                              │
│  → 代码审查工作流                    │
│  → 文档生成工作流                    │
│ │                                    │
│  快捷操作                            │
│  → 新建工作流                        │
│  → 查看后端配置                      │
│  → 查看设置                          │
│ └────────────────────────────────────┘
```

**实现:** Radix UI Dialog + 自定义搜索逻辑（fuzzy match）。后续可升级为 `cmdk` 库。

---

## 5. 新增功能清单

| 功能 | 优先级 | 依赖 | 说明 |
|------|--------|------|------|
| 可视化工作流编辑器 | P0 | React Flow | 拖拽编排 + 代码双向同步 |
| 自然语言快速启动 | P0 | 后端 NL→Lua API | 首页输入框，`luft run "xxx"` 的 GUI 版 |
| 全局搜索 / 命令面板 | P0 | — | Cmd+K，搜索 + 导航 + 命令 |
| WebSocket 实时事件流 | P0 | luft-daemon WS | 替换当前的模拟轮询 |
| 侧边栏导航 | P0 | — | 替换顶部导航 |
| 运行对比 | P1 | — | 两次运行结果并排对比 |
| Findings 聚合 | P1 | — | 跨运行汇总 Agent 发现的问题 |
| 工作流模板库 | P1 | — | 预设模板，一键导入 |
| 通知中心 | P2 | Browser Notification API | 运行完成/失败通知 |
| 亮色主题 | P2 | — | 暗色为主，亮色为辅 |
| 协作分享 | P2 | — | 工作流分享链接 |

---

## 6. 技术架构

### 6.1 前端架构变更

```
当前:
  mock-client.ts → React Query hooks → Pages

重设计:
  http-client.ts ─┐
                  ├─→ React Query hooks → Pages
  ws-client.ts ───┘
```

#### HTTP Client (`src/api/http-client.ts`)

REST API 客户端，负责：

- 运行列表 / 详情 / 启动
- 工作流 CRUD
- 统计数据
- 后端配置

```typescript
// 接口示例
interface HttpClient {
  runs: {
    list(params?: RunFilter): Promise<RunSummary[]>
    get(id: string): Promise<RunCheckpoint>
    start(req: StartRunRequest): Promise<RunSummary>
  }
  workflows: {
    list(): Promise<WorkflowSummary[]>
    get(id: string): Promise<Workflow>
    save(workflow: Workflow): Promise<void>
  }
  stats: {
    get(): Promise<DashboardStats>
  }
  backends: {
    list(): Promise<BackendConfig[]>
  }
}
```

#### WebSocket Client (`src/api/ws-client.ts`)

实时事件流客户端，负责：

- 运行进度推送
- Agent 事件流
- 阶段状态变更

```typescript
interface WsClient {
  connect(runId: string): WebSocket
  subscribe(event: string, handler: (data: unknown) => void): () => void
  disconnect(): void
}
```

连接地址: `ws://localhost:{port}/run?run_id={runId}` (luft-daemon)

#### React Query 集成

```typescript
// 运行详情 — HTTP 获取初始数据 + WS 推送增量更新
function useRunDetail(runId: string) {
  const query = useQuery({
    queryKey: ['run', runId],
    queryFn: () => httpClient.runs.get(runId),
  })

  // WS 增量更新
  useEffect(() => {
    const ws = wsClient.connect(runId)
    const unsub = ws.subscribe('checkpoint', (data) => {
      queryClient.setQueryData(['run', runId], data)
    })
    return () => { unsub(); ws.disconnect() }
  }, [runId])

  return query
}
```

### 6.2 新增依赖

| 包 | 用途 |
|----|------|
| `reactflow` (`@xyflow/react`) | 可视化工作流画布 |
| `cmdk` | 命令面板（或用 Radix Dialog 自建） |
| `@microsoft/fetch-event-source` | SSE 备选方案（如果不用 WS） |

### 6.3 新增 Zustand Store

```typescript
// src/stores/workflow-editor.ts
interface WorkflowEditorState {
  nodes: Node[]
  edges: Edge[]
  selectedNodeId: string | null
  dirty: boolean
  mode: 'visual' | 'code'
  luaCode: string
  // ...actions
}
```

```typescript
// src/stores/sidebar.ts
interface SidebarState {
  collapsed: boolean
  toggle: () => void
}
```

### 6.4 文件结构

```
web/src/
├── api/
│   ├── types.ts              # 已有，API 类型定义
│   ├── http-client.ts        # 新增：REST 客户端
│   ├── ws-client.ts          # 新增：WebSocket 客户端
│   ├── mock-client.ts        # 保留：开发环境 fallback
│   └── mock-data.ts          # 保留：开发环境 mock
├── components/
│   ├── layout/
│   │   ├── sidebar.tsx       # 新增：侧边栏导航
│   │   ├── command-palette.tsx # 新增：Cmd+K 搜索
│   │   └── top-nav.tsx       # 移除
│   ├── editor/
│   │   ├── workflow-canvas.tsx    # 新增：React Flow 画布
│   │   ├── node-panel.tsx         # 新增：节点拖拽面板
│   │   ├── node-property.tsx      # 新增：节点属性编辑
│   │   └── code-sync.ts           # 新增：Lua ↔ 可视化双向同步
│   ├── home/
│   │   ├── quick-start.tsx        # 新增：自然语言快速启动
│   │   ├── stats-overview.tsx     # 重命名自 StatCard
│   │   ├── recent-runs.tsx        # 新增
│   │   └── workflow-templates.tsx # 新增
│   ├── run/
│   │   ├── run-header.tsx         # 重构自 run-detail
│   │   ├── phase-timeline.tsx     # 新增：横向时间线
│   │   ├── agent-grid.tsx         # 新增：Agent 网格
│   │   ├── event-stream.tsx       # 已有，改为默认折叠
│   │   └── detail-drawer.tsx      # 已有，增加 Findings tab
│   └── ui/                        # 已有，shadcn 组件
├── pages/
│   ├── home.tsx                   # 新增（替代 dashboard.tsx）
│   ├── workflow-editor.tsx        # 新增（替代 workflows.tsx）
│   ├── run-detail.tsx             # 重构
│   ├── backends.tsx               # 保持
│   └── settings.tsx               # 新增
├── hooks/                         # 已有 + 新增 WS hooks
├── stores/
│   ├── ui.ts                      # 已有
│   ├── workflow-editor.ts         # 新增
│   └── sidebar.ts                 # 新增
└── styles/
    └── globals.css                # 更新色彩/字体
```

---

## 7. 后端 API 需求

当前 luft-daemon 只提供 WebSocket（`/mcp` 和 `/run`）。重设计需要新增 REST API 层。

### 7.1 必需的 REST 端点

| Method | Path | 说明 |
|--------|------|------|
| `GET` | `/api/runs` | 运行列表（支持 `?status=&limit=&sort=` 过滤） |
| `GET` | `/api/runs/:id` | 运行详情（checkpoint） |
| `POST` | `/api/runs` | 启动运行 |
| `GET` | `/api/runs/:id/events` | 运行事件流（SSE 或 WS） |
| `GET` | `/api/workflows` | 工作流列表 |
| `GET` | `/api/workflows/:id` | 工作流详情 |
| `PUT` | `/api/workflows/:id` | 保存工作流 |
| `GET` | `/api/stats` | 统计数据 |
| `GET` | `/api/backends` | 后端列表 |

### 7.2 WebSocket 端点（已有）

| Path | 说明 |
|------|------|
| `/run` | 运行执行协议（已有） |
| `/mcp` | MCP JSON-RPC（已有） |

新增 `subscribe` 消息类型，前端可以订阅特定运行的事件流：

```json
{
  "type": "subscribe",
  "run_id": "a3f7c"
}
```

---

## 8. 实施路线图

### Phase 1 — 基础重构（2 周）

**目标:** 导航重构 + 首页重设计 + 色彩系统切换

**任务清单:**

- [ ] `globals.css` 色彩/字体更新（teal 主色、新背景层）
- [ ] `Sidebar` 组件，替换 `TopNav`
- [ ] `home.tsx` 替换 `dashboard.tsx`
- [ ] `QuickStart` 组件（自然语言输入 + 选择器）
- [ ] `StatsOverview` 重构
- [ ] `RecentRuns` 面板
- [ ] `CommandPalette` (Cmd+K)
- [ ] 路由调整（移除 `/runs`，`/workflows` 改为 `/workflows/:id`）
- [ ] 网格背景纹理 + 玻璃态面板样式

**交付物:** 可点击的导航 + 首页 + 全局搜索，数据仍用 mock。

### Phase 2 — 工作流编辑器（3 周）

**目标:** 可视化编排 + 代码双向同步

**任务清单:**

- [ ] `reactflow` 集成
- [ ] `WorkflowCanvas` 组件（画布 + 节点 + 连线）
- [ ] `NodePanel` 组件（拖拽面板）
- [ ] `NodeProperty` 组件（属性编辑）
- [ ] `workflow-editor.ts` Zustand store
- [ ] Lua ↔ 可视化双向同步逻辑
- [ ] `workflow-editor.tsx` 页面（双模式切换）
- [ ] 工作流 CRUD API 对接（如后端就绪）

**交付物:** 完整的可视化工作流编辑器。

### Phase 3 — 实时连接（2 周）

**目标:** 替换 mock 数据，接入真实后端

**任务清单:**

- [ ] `http-client.ts` 实现
- [ ] `ws-client.ts` 实现
- [ ] React Query hooks 对接真实 API
- [ ] 运行详情页 WebSocket 实时事件流
- [ ] 移除 `useRunDetail` 的 `setInterval` 模拟
- [ ] 错误处理 + 重连逻辑
- [ ] mock 数据作为开发环境 fallback 保留

**交付物:** 前端连接真实 luft-daemon。

### Phase 4 — 增强功能（2 周）

**目标:** 运行对比 + Findings 聚合 + 模板库

**任务清单:**

- [ ] 运行对比页面（并排展示两次运行的差异）
- [ ] Findings 聚合面板（跨运行汇总，按严重性分类）
- [ ] 工作流模板库（预设模板选择 + 导入）
- [ ] DetailDrawer 增加 Findings tab
- [ ] 通知系统（运行完成/失败的浏览器通知）

**交付物:** 完整产品体验。

---

## 9. 设计决策记录

| 决策 | 选择 | 理由 |
|------|------|------|
| 导航方式 | 侧边栏 | 为编辑器释放水平空间；列表内嵌减少跳转 |
| 主色 | Teal `#14b8a6` | 亮绿刺眼，teal 适合长时间专业使用 |
| 编辑器模式 | 可视化 + 代码双模 | 兼顾低门槛（拖拽）和灵活性（代码） |
| 同步策略 | 代码优先 | 代码是 source of truth，可视化是投影 |
| 运行列表 | 首页内嵌 | 不独占页面，降低导航成本 |
| 事件流 | 默认折叠 | 按需展开，降低页面噪音 |
| 命令面板 | Cmd+K | 面向高频用户的标准范式 |
| 实时通信 | WebSocket | 已有 daemon WS 支持，双向通信 |

---

## 附录 A: 当前状态与重设计对比

| 维度 | 当前 | 重设计 |
|------|------|--------|
| 导航 | 顶部导航栏 | 可折叠侧边栏 |
| 页面数 | 5 个扁平页面 | 4 个页面 + 内嵌面板 |
| 主色 | `#00e676` (亮绿) | `#14b8a6` (teal) |
| 首页 | 统计 + 运行列表 | 快速启动 + 统计 + 最近运行 + 模板 |
| 工作流 | 列表 + 代码编辑器 | 可视化画布 + 代码双模 |
| 运行详情 | 左侧 timeline + 事件流 | 横向时间线 + Agent 网格 + 折叠事件流 |
| 数据源 | 全部 mock | HTTP + WebSocket |
| 搜索 | 无 | Cmd+K 全局命令面板 |
| 快速启动 | Run 对话框 | 首页自然语言输入框 |
