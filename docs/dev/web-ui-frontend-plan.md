# Luft Web UI 前端开发方案

> 状态：草案 v0.1 | 更新：2025-08-19
>
> 配套文档：
> - [Web UI 设计文档](./web-ui-design.md) — 视觉风格、布局、交互
> - [Web UI 后端开发方案](./web-ui-dev-plan.md) — API 契约、后端架构、总工期
>
> 本文档定义前端的技术栈、项目结构、数据流架构、组件规范和分阶段实现计划。

---

## 目录

1. [技术栈](#1-技术栈)
2. [项目结构](#2-项目结构)
3. [路由设计](#3-路由设计)
4. [数据流架构](#4-数据流架构)
5. [类型系统（前后端对齐）](#5-类型系统前后端对齐)
6. [组件规范与 shadcn/ui 映射](#6-组件规范与-shadcnui-映射)
7. [样式系统](#7-样式系统)
8. [分阶段实现计划](#8-分阶段实现计划)
9. [关键技术决策](#9-关键技术决策)
10. [开发环境搭建](#10-开发环境搭建)

---

## 1. 技术栈

| 技术 | 版本 | 用途 | 选型理由 |
|------|------|------|----------|
| **React** | 19 | UI 框架 | 生态成熟，shadcn/ui 基础 |
| **Vite** | 6 | 构建/开发服务器 | 极速 HMR，零配置 TypeScript |
| **TypeScript** | 5.7 | 类型系统 | 安全性，API 类型对齐 |
| **shadcn/ui** | latest | 组件库 | 基于 Radix，代码直接拷入项目，完全可控 |
| **Tailwind CSS** | 4 | 样式系统 | shadcn/ui 依赖，原子化 CSS |
| **lucide-react** | latest | 图标库 | shadcn/ui 默认搭配，tree-shakeable，1400+ 图标 |
| **React Router** | 7 | 路由 | 嵌套路由，loader/action 数据流 |
| **TanStack Query** | 5 | 服务端状态管理 | 缓存/重试/轮询，替代手写 fetch 逻辑 |
| **Zustand** | 5 | 客户端状态管理 | 轻量，适合 UI 局部状态（密度切换、侧栏折叠等） |
| **CodeMirror 6** | 6 | Lua 代码编辑器 | 轻量、可扩展，支持语法高亮 |

### 1.1 图标库：lucide-react

| 对比项 | lucide-react ✅ | react-icons | heroicons |
|--------|-----------------|-------------|-----------|
| 图标数量 | 1400+ | 超大（多集合） | ~300 |
| Tree-shaking | ✅ 按需导入 | ⚠️ 需注意子路径 | ✅ |
| 风格 | 线条一致，2px stroke | 风格混杂 | 双风格(outline/solid) |
| shadcn 集成 | 原生默认 | 需手动替换 | 需手动替换 |
| 运行时体积 | 极小（per-icon） | 中等 | 极小 |

> lucide-react 是 shadcn/ui 所有组件的默认图标，无需适配；线条风格统一，契合可观测平台的专业气质。

### 1.2 依赖清单

```bash
# 核心框架
react react-dom
react-router-dom
@tanstack/react-query

# 状态管理
zustand

# 组件库 + 样式
tailwindcss @tailwindcss/vite
class-variance-authority clsx tailwind-merge
lucide-react

# 代码编辑器
@uiw/react-codemirror @codemirror/lang-lua

# 开发依赖
typescript @types/react @types/react-dom
@vitejs/plugin-react vite
```

---

## 2. 项目结构

```
web/
├── package.json
├── vite.config.ts
├── tsconfig.json
├── tsconfig.app.json
├── components.json              # shadcn/ui 配置
├── index.html
├── public/
└── src/
    ├── main.tsx                 # 入口：React + Router + QueryClient
    ├── App.tsx                  # 路由定义 + 全局 Layout
    │
    ├── api/                     # API 层
    │   ├── client.ts            # fetch 封装（baseURL、错误处理）
    │   ├── types.ts             # 后端类型镜像（手动对齐 serde）
    │   ├── runs.ts              # Run 相关 API 函数
    │   ├── workflows.ts         # Workflow 相关 API 函数
    │   ├── backends.ts          # Backend 相关 API 函数
    │   └── ws.ts                # WebSocket 连接管理器
    │
    ├── hooks/                   # 自定义 Hooks
    │   ├── useRuns.ts           # Run 列表（TanStack Query）
    │   ├── useRunDetail.ts      # Run 详情（TanStack Query）
    │   ├── useRunEvents.ts      # WS 事件流（WebSocket + 状态）
    │   └── useDashboardStats.ts # Dashboard 统计
    │
    ├── stores/                  # Zustand stores
    │   └── ui.ts                # UI 状态（密度、侧栏、暂停、对话框）
    │
    ├── components/              # 组件
    │   ├── ui/                  # shadcn/ui 基础组件（CLI 生成）
    │   ├── layout/
    │   │   ├── top-nav.tsx      # 全局导航栏
    │   │   ├── breadcrumb.tsx   # 面包屑（复用 shadcn）
    │   │   └── page-shell.tsx   # 页面外壳（max-width + padding）
    │   ├── status-badge.tsx     # 状态徽章（5 种状态）
    │   ├── progress-bar.tsx     # 进度条（Phase 级）
    │   ├── agent-card.tsx       # Agent 卡片（compact/comfortable）
    │   ├── event-stream.tsx     # 事件流（实时滚动 + 暂停）
    │   ├── phase-accordion.tsx  # Phase 手风琴
    │   ├── stat-card.tsx        # 统计数字卡片
    │   ├── run-dialog.tsx       # 发起 Run 对话框
    │   ├── detail-drawer.tsx    # Agent 详情抽屉
    │   └── code-editor.tsx      # CodeMirror Lua 编辑器封装
    │
    ├── pages/                   # 页面组件
    │   ├── dashboard.tsx
    │   ├── runs.tsx
    │   ├── run-detail.tsx
    │   ├── workflows.tsx
    │   └── backends.tsx
    │
    ├── lib/
    │   ├── format.ts            # 格式化（token、时间、duration）
    │   └── event-utils.ts       # 事件流辅助（分类、缩进、过滤）
    │
    └── styles/
        └── globals.css          # Tailwind 指令 + CSS 变量（设计 Token）
```

---

## 3. 路由设计

### 3.1 路由表

| 路径 | 组件 | 说明 |
|------|------|------|
| `/` | `Dashboard` | 全局总览（活跃 Run + 最近完成 + 统计） |
| `/runs` | `RunsList` | Run 列表（筛选/搜索） |
| `/runs/:runId` | `RunDetail` | Run 详情（实时监控） |
| `/workflows` | `Workflows` | Workflow 列表 + 编辑器 |
| `/workflows/:name` | `Workflows` | 编辑指定 Workflow（同组件，路由参数切换） |
| `/backends` | `Backends` | Backend 配置管理 |

### 3.2 路由守卫

```typescript
// App.tsx
function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <Routes>
          <Route element={<RootLayout />}>
            <Route index element={<Dashboard />} />
            <Route path="runs" element={<RunsList />} />
            <Route path="runs/:runId" element={<RunDetail />} />
            <Route path="workflows" element={<Workflows />} />
            <Route path="workflows/:name" element={<Workflows />} />
            <Route path="backends" element={<Backends />} />
            <Route path="*" element={<NotFound />} />
          </Route>
        </Routes>
      </BrowserRouter>
    </QueryClientProvider>
  );
}
```

`RootLayout` 包含 TopNav + `<Outlet />`，所有页面共享导航栏。

### 3.3 页面跳转

- **Run 列表 → 详情**：`<Link to={`/runs/${run.run_id}`}>` 或 `useNavigate()`
- **Dashboard → 详情**：卡片点击跳转
- **详情 → 列表**：面包屑 + 返回按钮
- **RunDialog → 详情**：发起 Run 后 `navigate(`/runs/${data.run_id}`)`

---

## 4. 数据流架构

### 4.1 三层状态管理

```
┌─────────────────────────────────────────────────────┐
│                    React 组件                        │
│                        │                             │
│          ┌─────────────┼─────────────┐              │
│          ▼             ▼             ▼              │
│   TanStack Query   Zustand     useRunEvents        │
│   (服务端状态)     (UI 状态)    (WebSocket)         │
│          │             │             │              │
│          ▼             │             ▼              │
│     fetch() / API      │       WebSocket            │
│          │             │             │              │
└──────────┼─────────────┼─────────────┼──────────────┘
           ▼             ▼             ▼
        axum HTTP                  axum WS
```

三层各司其职，无交叉依赖。

### 4.2 TanStack Query — 服务端状态

负责所有 HTTP 请求的缓存、重试、失效管理。

```typescript
// hooks/useRuns.ts
export function useRuns(filters?: RunFilters) {
  return useQuery({
    queryKey: ['runs', filters],
    queryFn: () => api.runs.list(filters),
    refetchInterval: (query) => {
      // 有运行中的 run 时，10s 轮询刷新列表
      return query.state.data?.runs.some(r => r.status === 'running')
        ? 10_000 : false;
    },
  });
}

// hooks/useRunDetail.ts
export function useRunDetail(runId: string) {
  return useQuery({
    queryKey: ['run', runId],
    queryFn: () => api.runs.get(runId),
    enabled: !!runId,
  });
}

// 发起 Run（mutation）
export function useStartRun() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (req: StartRunRequest) => api.runs.start(req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['runs'] });
    },
  });
}
```

**QueryClient 配置：**

```typescript
// main.tsx
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,        // 30s 内不重新获取
      retry: 2,                 // 失败重试 2 次
      refetchOnWindowFocus: true, // 窗口聚焦时刷新
    },
  },
});
```

**缓存失效策略：**

| 事件 | 失效的 Query |
|------|-------------|
| 发起 Run | `['runs']` |
| Run 完成（WS 通知） | `['runs']`, `['run', runId]` |
| 保存 Workflow | `['workflows']`, `['workflow', name]` |
| 更新 Backend | `['backends']` |

### 4.3 Zustand — 客户端 UI 状态

```typescript
// stores/ui.ts
import { create } from 'zustand';

interface UIState {
  // Run 详情页
  agentCardDensity: 'compact' | 'comfortable';
  sidebarCollapsed: boolean;
  eventStreamPaused: boolean;
  selectedPhaseId: number | null;

  // 全局
  runDialogOpen: boolean;

  // actions
  toggleDensity: () => void;
  toggleSidebar: () => void;
  toggleEventPause: () => void;
  setSelectedPhase: (id: number | null) => void;
  setRunDialogOpen: (open: boolean) => void;
}

export const useUIStore = create<UIState>((set) => ({
  agentCardDensity: 'compact',
  sidebarCollapsed: false,
  eventStreamPaused: false,
  selectedPhaseId: null,
  runDialogOpen: false,

  toggleDensity: () =>
    set((s) => ({
      agentCardDensity: s.agentCardDensity === 'compact' ? 'comfortable' : 'compact',
    })),
  toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
  toggleEventPause: () => set((s) => ({ eventStreamPaused: !s.eventStreamPaused })),
  setSelectedPhase: (id) => set({ selectedPhaseId: id }),
  setRunDialogOpen: (open) => set({ runDialogOpen: open }),
}));
```

### 4.4 useRunEvents — WebSocket Hook

```typescript
// hooks/useRunEvents.ts
import { useEffect, useRef, useState } from 'react';
import { useUIStore } from '@/stores/ui';
import type { AgentEvent } from '@/api/types';

export function useRunEvents(runId: string) {
  const [events, setEvents] = useState<AgentEvent[]>([]);
  const [connected, setConnected] = useState(false);
  const paused = useUIStore((s) => s.eventStreamPaused);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;

  useEffect(() => {
    const ws = new WebSocket(`${WS_BASE}/ws/runs/${runId}`);

    ws.onopen = () => setConnected(true);
    ws.onclose = () => setConnected(false);
    ws.onmessage = (e) => {
      if (pausedRef.current) return; // 暂停时不追加（用 ref 避免重连）
      const event = JSON.parse(e.data) as AgentEvent;
      setEvents((prev) => [...prev, event]);
    };

    return () => ws.close();
  }, [runId]); // 只在 runId 变化时重连，不依赖 paused

  return { events, connected };
}
```

> **注意：** paused 用 `useRef` 而非直接依赖，避免暂停切换导致 WebSocket 重连。

### 4.5 API 客户端封装

```typescript
// api/client.ts
const API_BASE = '/api';

class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

async function request<T>(
  path: string,
  options?: RequestInit,
): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    ...options,
    headers: {
      'Content-Type': 'application/json',
      ...options?.headers,
    },
  });

  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: res.statusText }));
    throw new ApiError(res.status, body.error ?? 'Unknown error');
  }

  return res.status === 204 ? (undefined as T) : res.json();
}

export const http = {
  get: <T>(path: string) => request<T>(path),
  post: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: 'POST', body: body ? JSON.stringify(body) : undefined }),
  put: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: 'PUT', body: body ? JSON.stringify(body) : undefined }),
  delete: <T>(path: string) => request<T>(path, { method: 'DELETE' }),
};
```

---

## 5. 类型系统（前后端对齐）

手动维护 `api/types.ts` 对齐 Rust serde 输出。

```typescript
// api/types.ts

// ── 基础类型 ──
type RunId = string;
type AgentId = string;
type PhaseId = number;

// ── Token ──
interface TokenUsage {
  input: number;
  output: number;
}

// ── 状态枚举 ──
type RunStatus = 'running' | 'completed' | 'failed' | 'cancelled';
type AgentStatus = 'pending' | 'running' | 'done' | 'failed';
type CheckpointStatus = 'running' | 'completed' | 'failed' | 'cancelled';

// ── Run 列表项 ──
interface RunSummary {
  run_id: RunId;
  run_dir: string;
  task: string;
  status: RunStatus;
  current_phase: number;
  total_phases: number;
  total_tokens: number;
  started_at: string;       // ISO 8601
  elapsed_ms: number;
}

interface RunsResponse {
  runs: RunSummary[];
  total: number;
}

// ── Run 详情（Checkpoint） ──
interface PhaseSummary {
  phase_id: PhaseId;
  label: string;
  ok: number;
  failed: number;
}

interface AgentResultCache {
  status: AgentStatus;
  tokens: TokenUsage;
  elapsed_ms: number;
}

interface Finding {
  id: string;
  severity: string;
  message: string;
  source?: AgentId;
}

interface RunCheckpoint {
  run_id: RunId;
  task: string;
  status: CheckpointStatus;
  current_phase: number;
  completed_phases: PhaseSummary[];
  agent_results: Record<AgentId, AgentResultCache>;
  findings: Finding[];
  total_tokens: number;
}

// ── 事件（WebSocket / 历史） ──
interface ProgressDelta {
  tokens?: Partial<TokenUsage>;
  content?: string;
}

type AgentEvent =
  | { type: 'RunStarted'; run_id: RunId; task: string; ts: string }
  | { type: 'PhaseStarted'; run_id: RunId; phase_id: PhaseId; label: string; planned: number }
  | { type: 'AgentStarted'; run_id: RunId; phase_id: PhaseId; agent_id: AgentId; prompt_preview: string; model?: string }
  | { type: 'AgentProgress'; run_id: RunId; agent_id: AgentId; delta: ProgressDelta }
  | { type: 'AgentDone'; run_id: RunId; agent_id: AgentId; status: AgentStatus; tokens: TokenUsage; elapsed_ms: number }
  | { type: 'PhaseDone'; run_id: RunId; phase_id: PhaseId; ok: number; failed: number }
  | { type: 'RunDone'; run_id: RunId; status: RunStatus; total_tokens: TokenUsage; report: unknown };

// ── 请求体 ──
interface StartRunRequest {
  workflow: string;
  task: string;
  backend: string;
  args?: Record<string, unknown>;
}

interface StartRunResponse {
  run_id: RunId;
  run_dir: string;
  status: 'running';
  ws_url: string;
}

// ── Dashboard 统计 ──
interface DashboardStats {
  today_runs: number;
  today_tokens: number;
  today_success: number;
  today_failed: number;
  active_runs: RunSummary[];
  recent_runs: RunSummary[];
}

// ── Workflow ──
interface WorkflowSummary {
  name: string;
  description: string;
  phases: number;
  agents: number;
}

interface WorkflowDetail {
  name: string;
  content: string;          // Lua 源码
  description: string;
}

// ── Backend ──
interface BackendConfig {
  id: string;
  name: string;
  provider: string;         // anthropic / openai / ollama
  model: string;
  connected: boolean;
  usage_count: number;
}
```

---

## 6. 组件规范与 shadcn/ui 映射

### 6.1 shadcn/ui 组件安装清单

```bash
npx shadcn@latest add badge button card accordion table \
  breadcrumb dialog sheet sonner select input tabs \
  tooltip separator scroll-area skeleton dropdown-menu
```

### 6.2 组件映射表

| 设计文档组件 | shadcn 基础 | 自定义程度 | 说明 |
|-------------|------------|-----------|------|
| TopNav | 自建（`nav` + `Link`） | 高 | 固定顶部，active 高亮 |
| StatusBadge | `Badge` + 自定义 variant | 中 | 5 种状态色，背景 12% 透明 |
| ProgressBar | `Progress` | 低 | 直接用，加 shimmer 动画 |
| AgentCard | `Card` | 高 | 内部结构全定制，角色标签 + 状态 |
| EventStream | 自建 | 高 | Terminal 风格，等宽字体，实时滚动 |
| PhaseAccordion | `Accordion` | 中 | 默认展开运行中的 Phase |
| StatCard | `Card` | 中 | 大数字 + 标签 |
| DataColumn（表格） | `Table` | 低 | 固定表头，行 hover |
| BreadcrumbNav | `Breadcrumb` | 低 | 直接用 |
| RunDialog | `Dialog` | 中 | 表单：workflow + task + backend |
| DetailDrawer | `Sheet`（right） | 低 | Agent 详情抽屉 |
| Toast | `Sonner` | 低 | shadcn 推荐 |
| CodeEditor | CodeMirror 6 | 高 | Lua 高亮 + 暗色主题 |
| 筛选下拉 | `Select` | 低 | 状态/时间筛选 |
| 搜索框 | `Input` | 低 | 实时过滤 |
| 密度切换 | `DropdownMenu` | 低 | Compact / Comfortable |

### 6.3 核心组件实现规范

#### StatusBadge

```tsx
// components/status-badge.tsx
const statusConfig = {
  running:   { icon: Loader,  label: '运行中', className: 'bg-blue-500/12 text-blue-400' },
  completed: { icon: Check,   label: '完成',   className: 'bg-emerald-500/12 text-emerald-400' },
  failed:    { icon: X,       label: '失败',   className: 'bg-red-500/12 text-red-400' },
  pending:   { icon: Circle,  label: '等待中', className: 'bg-gray-500/12 text-gray-400' },
  cancelled: { icon: Ban,     label: '已取消', className: 'bg-amber-500/12 text-amber-400' },
} as const;

function StatusBadge({ status }: { status: keyof typeof statusConfig }) {
  const cfg = statusConfig[status];
  const Icon = cfg.icon;
  return (
    <span className={cn('inline-flex items-center gap-1 rounded px-2 py-0.5 text-xs', cfg.className)}>
      <Icon className={cn('h-3 w-3', status === 'running' && 'animate-pulse')} />
      {cfg.label}
    </span>
  );
}
```

#### AgentCard

```tsx
// components/agent-card.tsx
const roleColors = {
  producer:  'bg-blue-500/12 text-blue-400',
  adversary: 'bg-amber-500/12 text-amber-400',
  voter:     'bg-purple-500/12 text-purple-400',
  default:   'bg-gray-500/12 text-gray-400',
};

interface AgentCardProps {
  agentId: string;
  role?: string;
  status: AgentStatus;
  tokens?: TokenUsage;
  elapsedMs?: number;
  density?: 'compact' | 'comfortable';
  onClick?: () => void;
}

// Compact: 200×100px — 角色标签 + 状态图标 + token + 耗时
// Comfortable: 240×160px — 上述 + prompt 前 2 行 + output 摘要 1 行
```

#### EventStream

```tsx
// components/event-stream.tsx
// 功能：
// - 等宽字体渲染事件列表
// - 自动滚动到底部（sticky bottom）
// - 暂停按钮（从 Zustand 读取）
// - 事件行缩进：Run 级 0、Phase 级 2、Agent 级 4 空格
// - 时间戳格式化
// - 图标：▶ 开始、← 完成、→ 进度
```

### 6.4 页面组件结构

```
Dashboard
├─ TopNav（全局）
├─ ActiveRunCards（活跃 Run 卡片网格）
│  └─ StatCard × N
├─ RecentRunsList（最近完成）
└─ StatsPanel（今日统计）
   └─ StatCard × 4

RunsList
├─ TopNav
├─ FilterBar（状态 Select + 时间 Select + 搜索 Input）
└─ Table（Run 列表）
   └─ StatusBadge + ProgressBar per row

RunDetail
├─ TopNav + Breadcrumb
├─ RunHeader（任务名 + 状态 + token + 计时 + 进度条）
├─ 左侧栏（240px）
│  ├─ Timeline（Phase 列表）
│  ├─ PhaseInfo
│  ├─ FindingsCount
│  └─ ReportStatus
├─ 主区域
│  ├─ PhaseAccordion × N
│  │  └─ AgentCard × M（per Phase）
│  └─ EventStream（底部可折叠）
└─ DetailDrawer（右侧滑出，点击 Agent 卡片触发）

Workflows
├─ TopNav
├─ 左栏（260px）：Workflow 列表 + 新建按钮
└─ 右栏：元信息卡片 + CodeEditor + 操作栏

Backends
├─ TopNav
└─ Backend 卡片网格
   └─ 每张卡片：名称 + Provider + Model + 状态 + 操作按钮
```

---

## 7. 样式系统

### 7.1 CSS 变量（与设计文档对齐）

```css
/* src/styles/globals.css */
@import "tailwindcss";

:root {
  /* 背景层级 */
  --bg-base:      #0a0e14;
  --bg-surface:   #141821;
  --bg-elevated:  #1c2230;
  --bg-hover:     #242b3d;

  /* 文本 */
  --text-primary:   #e4e7eb;
  --text-secondary: #8b95a7;
  --text-muted:     #5c6578;

  /* 强调 */
  --accent:     #00e676;
  --accent-dim: rgba(0, 230, 118, 0.12);

  /* 状态 */
  --status-running:   #3b82f6;
  --status-success:   #00e676;
  --status-failed:    #ef4444;
  --status-pending:   #6b7280;
  --status-cancelled: #f59e0b;

  /* 边框 */
  --border:       #2a3142;
  --border-focus: rgba(0, 230, 118, 0.5);
}

body {
  background-color: var(--bg-base);
  color: var(--text-primary);
  font-family: 'Inter', system-ui, sans-serif;
}
```

### 7.2 字体加载

```html
<!-- index.html -->
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@500;700&family=IBM+Plex+Mono:wght@400;500&family=Inter:wght@400;500;600&display=swap" rel="stylesheet">
```

| 字体 | 用途 | Tailwind 映射 |
|------|------|--------------|
| Space Grotesk | 标题/UI | `font-display` |
| IBM Plex Mono | 数据/代码 | `font-mono` |
| Inter | 正文 | 默认 |

### 7.3 Tailwind 扩展配置

```css
/* globals.css（续） */
@theme {
  --font-display: 'Space Grotesk', system-ui, sans-serif;
  --font-mono: 'IBM Plex Mono', 'JetBrains Mono', monospace;
  --color-bg-base: #0a0e14;
  --color-bg-surface: #141821;
  --color-bg-elevated: #1c2230;
  --color-accent: #00e676;
}
```

### 7.4 动画

```css
/* 运行中图标脉冲 */
@keyframes pulse-dot {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}
.animate-pulse-dot { animation: pulse-dot 1.5s ease-in-out infinite; }

/* 进度条 shimmer */
@keyframes shimmer {
  0% { background-position: -200% 0; }
  100% { background-position: 200% 0; }
}
.shimmer {
  background: linear-gradient(90deg, transparent, rgba(0,230,118,0.15), transparent);
  background-size: 200% 100%;
  animation: shimmer 2s linear infinite;
}
```

---

## 8. 分阶段实现计划

### Phase 0：项目搭建（1 天）

**目标**：Vite + React + shadcn 骨架跑通。

**任务：**
- `npm create vite@latest web -- --template react-ts`
- 安装 Tailwind CSS 4 + 初始化 shadcn/ui
- 配置 Vite proxy（`/api` → `localhost:3000`，`/ws` → WS）
- 搭建路由骨架（React Router 7，5 个空白页面）
- 实现 TopNav 组件
- 应用 globals.css（CSS 变量 + 暗色主题）
- 实现 PageShell 布局容器

**验收：**
- `npm run dev` 启动，5 个页面路由可切换
- TopNav 高亮当前页
- 暗色主题正确渲染

---

### Phase 1：Runs 列表页（1-2 天）

**目标**：显示历史 Run 列表，点击进详情。

**任务：**
- 实现 `api/client.ts`（fetch 封装）
- 实现 `api/types.ts`（类型定义）
- 实现 `api/runs.ts`（list / get / getEvents 等）
- 实现 `useRuns` Hook
- 实现 FilterBar（状态 Select + 时间 Select + 搜索 Input）
- 实现 Runs 表格（StatusBadge + ProgressBar + 数据格式化）
- 实现 `lib/format.ts`（token 格式化、时间格式化）

**验收：**
- 列表正确加载并显示所有历史 Run
- 筛选和搜索功能正常
- 行 hover 高亮，点击跳转详情

---

### Phase 2：Run 详情页（只读）（2-3 天）

**目标**：展示 Run 的 Phase 结构、Agent 结果、事件历史。

**任务：**
- 实现 `useRunDetail` Hook
- 实现 RunHeader（任务名 + 状态 + token + 进度）
- 实现左侧栏（Timeline + PhaseInfo + Findings + Report）
- 实现 PhaseAccordion（展开/折叠，默认展开当前 Phase）
- 实现 AgentCard（Compact 模式）
- 实现 AgentCard 详情 DetailDrawer（Sheet）
- 实现 EventStream（从 API 加载历史事件，非实时）
- 实现 `lib/event-utils.ts`（事件分类、缩进、图标映射）
- 实现面包屑导航
- 实现密度切换

**验收：**
- 正确显示 Phase → Agent 层级
- Agent 卡片状态/token/耗时正确
- 点击 Agent 卡片弹出详情抽屉
- 事件流正确显示历史事件
- 与 CLI `luft status` 数据一致

---

### Phase 3：实时事件流（WebSocket）（2 天）

**目标**：运行中的 Run 实时展示进度。

**任务：**
- 实现 `api/ws.ts`（WebSocket 连接管理器）
- 实现 `useRunEvents` Hook（WS 连接 + 事件追加 + 暂停）
- 升级 EventStream 支持实时事件追加
- AgentCard 运行态动效（脉冲、token 滚动）
- 进度条实时更新 + shimmer 动画
- Runs 列表中运行行自动刷新（TanStack Query 轮询）
- 实现暂停/恢复事件流功能
- WS 连接状态指示器

**验收：**
- 发起 Run 后，详情页实时看到事件追加
- Agent 卡片从 pending → running → done 转换正确
- 暂停/恢复正常工作，不导致 WS 重连
- Run 完成后 WS 自动关闭

---

### Phase 4：Dashboard + RunDialog（2 天）

**目标**：首屏总览 + 发起新任务。

**任务：**
- 实现 `useDashboardStats` Hook
- 实现 Dashboard 页（活跃卡片 + 最近完成 + 统计面板）
- 实现 StatCard 组件
- 实现 RunDialog（Dialog 表单：workflow + task + backend）
- 实现 `useStartRun` mutation
- 实现 Toast 通知（Sonner）
- 发起 Run 后跳转详情页

**验收：**
- Dashboard 正确显示活跃和最近完成的 Run
- RunDialog 发起 Run 后跳转到详情页
- 统计数字与实际数据一致

---

### Phase 5：Workflows 编辑器（2-3 天）

**目标**：在线编辑和保存 Lua 工作流。

**任务：**
- 实现 `api/workflows.ts`
- 实现 Workflows 页（左栏列表 + 右栏编辑器）
- 实现 CodeEditor（CodeMirror 6 + Lua 语法高亮 + 暗色主题）
- 实现元信息卡片（Phase/Agent 统计）
- 实现保存/另存为/试运行
- 试运行 → 发起 Run → 跳转详情

**验收：**
- 能正确加载现有 .lua 工作流
- 编辑后保存生效
- 试运行发起 Run 并跳转

---

### Phase 6：Backends 管理 + 打磨（1-2 天）

**目标**：Backend 配置 + 全局细节打磨。

**任务：**
- 实现 `api/backends.ts`
- 实现 Backends 卡片网格
- 实现 Backend 编辑表单
- 加载状态（Skeleton 组件）
- 空状态、错误状态展示
- 错误边界（ErrorBoundary）
- 响应式适配（Tablet 断点）
- Mobile 可读性检查

**验收：**
- 能查看和编辑 Backend 配置
- 所有空/加载/错误状态合理展示
- Tablet 宽度下布局合理

---

### 工期汇总

| Phase | 内容 | 预估工时 |
|-------|------|----------|
| 0 | 项目搭建 | 1 天 |
| 1 | Runs 列表 | 1-2 天 |
| 2 | Run 详情（只读） | 2-3 天 |
| 3 | 实时事件流 | 2 天 |
| 4 | Dashboard + RunDialog | 2 天 |
| 5 | Workflows 编辑器 | 2-3 天 |
| 6 | Backends + 打磨 | 1-2 天 |
| **合计** | | **11-15 天** |

---

## 9. 关键技术决策

### 9.1 为什么用 TanStack Query 而非 Redux/Context

| 维度 | TanStack Query ✅ | Redux Toolkit | React Context |
|------|-------------------|---------------|---------------|
| 服务端状态缓存 | 内置 | 需手写 | 需手写 |
| 自动重新获取 | 内置（窗口聚焦、间隔） | 需手写 | 不支持 |
| 加载/错误状态 | 内置 `isLoading/isError` | 需手写 | 需手写 |
| 请求去重 | 内置 | 需手写 | 不支持 |
| 代码量 | 极少 | 大 | 中 |

> Luft 的数据以服务端状态为主（Run 列表、详情、事件），TanStack Query 专为这种场景设计。

### 9.2 为什么手动类型对齐而非自动生成

- 类型数量可控（约 15 个核心类型）
- 避免 utoipa/swagger-codegen 的工具链复杂度
- 后期类型漂移成为问题时，可引入 `utoipa` + OpenAPI 自动生成

### 9.3 为什么用 CodeMirror 6 而非 Monaco

| 维度 | CodeMirror 6 ✅ | Monaco |
|------|-----------------|--------|
| 体积 | ~150KB（按需加载语言） | ~3MB+ |
| Lua 支持 | 第三方语法包 | 内置 |
| 暗色主题 | 易定制 | 固定风格 |
| Vite 集成 | 无需 web worker 配置 | 需要配置 worker |

> Luft 只编辑 Lua，不需要 Monaco 的多语言 IntelliSense。

### 9.4 为什么 WebSocket paused 用 ref 而非依赖

```typescript
// ❌ 错误：paused 变化导致 WS 重连
useEffect(() => { /* connect */ }, [runId, paused]);

// ✅ 正确：用 ref 避免重连
const pausedRef = useRef(paused);
useEffect(() => { /* connect */ }, [runId]);
```

暂停切换不应导致 WebSocket 断开重连，否则会丢失事件。

---

## 10. 开发环境搭建

### 10.1 项目初始化

```bash
# 1. 创建 Vite 项目
npm create vite@latest web -- --template react-ts
cd web

# 2. 安装 Tailwind CSS 4
npm install tailwindcss @tailwindcss/vite

# 3. 初始化 shadcn/ui
npx shadcn@latest init

# 4. 安装核心依赖
npm install react-router-dom @tanstack/react-query zustand lucide-react

# 5. 安装代码编辑器
npm install @uiw/react-codemirror @codemirror/lang-lua

# 6. 安装 shadcn 组件
npx shadcn@latest add badge button card accordion table \
  breadcrumb dialog sheet sonner select input tabs \
  tooltip separator scroll-area skeleton dropdown-menu
```

### 10.2 Vite 配置

```typescript
// web/vite.config.ts
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import path from 'path';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://localhost:3000',
      '/ws': {
        target: 'ws://localhost:3000',
        ws: true,
      },
    },
  },
});
```

### 10.3 TypeScript 配置

```json
// tsconfig.app.json（关键部分）
{
  "compilerOptions": {
    "target": "ES2020",
    "lib": ["ES2020", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "moduleResolution": "bundler",
    "jsx": "react-jsx",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "baseUrl": ".",
    "paths": {
      "@/*": ["./src/*"]
    }
  }
}
```

### 10.4 shadcn/ui 配置

```json
// components.json
{
  "$schema": "https://ui.shadcn.com/schema.json",
  "style": "new-york",
  "rsc": false,
  "tsx": true,
  "tailwind": {
    "config": "",
    "css": "src/styles/globals.css",
    "baseColor": "zinc",
    "cssVariables": true
  },
  "aliases": {
    "components": "@/components",
    "utils": "@/lib/utils",
    "ui": "@/components/ui"
  }
}
```

### 10.5 启动命令

```bash
# 终端 1：后端（需要先实现 luft serve，见后端方案）
cargo run -- serve --port 3000

# 终端 2：前端开发服务器
cd web && npm run dev

# 浏览器访问
open http://localhost:5173
```

### 10.6 生产构建

```bash
# 前端构建
cd web && npm run build    # → web/dist/

# 后端编译（嵌入静态资源，见后端方案 Phase 6）
cargo build --release
```

---

## 附录：前后端协作约定

| 约定 | 说明 |
|------|------|
| API 前缀 | 所有 HTTP 端点 `/api/*`，WebSocket `/ws/*` |
| 时间格式 | ISO 8601 字符串（`chrono` serde 默认） |
| ID 格式 | UUID v7 字符串 |
| 事件序列化 | `AgentEvent` serde enum，tag = variant 名（如 `"RunStarted"`） |
| 错误格式 | `{ "error": "message", "code": "optional" }` |
| 分页 | `?limit=20&offset=0`，响应含 `total` |
| WS 心跳 | 服务端每 30s 发 `{ "type": "ping" }`，客户端 60s 无响应断开 |
