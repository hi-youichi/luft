-- web-redesign.lua
-- 多阶段工作流：重新设计 Luft Web Dashboard
--
-- 架构：
--   Phase 1: 审计当前代码库，识别所有 gap，生成详细实施计划
--   Phase 2: 并行实现各模块（API 层、组件、页面、基础设施）
--   Phase 3: 验证构建 + 修复错误
--
-- 运行方式：
--   cargo run -- run -w workflows/web-redesign.lua -b opencode

meta = {
  reasoning = "Web UI 重设计：审计→并行实现→验证，覆盖 API 层、组件库、页面结构、实时通信",
  phases = {
    { label = "audit", description = "审计当前代码库并生成实施计划", dynamic = false },
    { label = "implement-api", description = "实现真实 API 客户端层", agents = 4, dynamic = true },
    { label = "implement-pages", description = "实现页面组件", agents = 4, dynamic = true },
    { label = "implement-components", description = "实现通用 UI 组件", agents = 3, dynamic = true },
    { label = "verify", description = "验证构建并修复错误", dynamic = false },
  },
}

----------------------------------------------------------------------
-- Schema 定义
----------------------------------------------------------------------

local AUDIT_SCHEMA = {
  type = "object",
  properties = {
    summary = { type = "string" },
    files = {
      type = "array",
      items = {
        type = "object",
        properties = {
          path = { type = "string" },
          lines = { type = "integer" },
          purpose = { type = "string" },
          issues = { type = "array", items = { type = "string" } },
          status = { type = "string", enum = { "keep", "modify", "rewrite", "delete" } },
        },
        required = { "path", "purpose", "status" },
      },
    },
    gaps = {
      type = "array",
      items = {
        type = "object",
        properties = {
          category = { type = "string" },
          description = { type = "string" },
          priority = { type = "string", enum = { "P0", "P1", "P2" } },
          effort = { type = "string", enum = { "small", "medium", "large" } },
        },
        required = { "category", "description", "priority", "effort" },
      },
    },
    api_endpoints = {
      type = "array",
      items = {
        type = "object",
        properties = {
          method = { type = "string" },
          path = { type = "string" },
          purpose = { type = "string" },
        },
        required = { "method", "path", "purpose" },
      },
    },
    plan = {
      type = "object",
      properties = {
        total_files = { type = "integer" },
        keep = { type = "integer" },
        modify = { type = "integer" },
        rewrite = { type = "integer" },
        new_files = { type = "integer" },
        estimated_phases = { type = "array", items = { type = "string" } },
      },
      required = { "total_files", "estimated_phases" },
    },
  },
  required = { "summary", "files", "gaps", "api_endpoints", "plan" },
}

local IMPLEMENT_RESULT = {
  type = "object",
  properties = {
    task = { type = "string" },
    files_created = { type = "array", items = { type = "string" } },
    files_modified = { type = "array", items = { type = "string" } },
    total_lines = { type = "integer" },
    summary = { type = "string" },
    errors = { type = "array", items = { type = "string" } },
  },
  required = { "task", "summary" },
}

local VERIFY_RESULT = {
  type = "object",
  properties = {
    success = { type = "boolean" },
    output = { type = "string" },
    errors = { type = "array", items = { type = "string" } },
    fixes_applied = { type = "array", items = { type = "string" } },
  },
  required = { "success", "output" },
}

----------------------------------------------------------------------
-- 工具函数
----------------------------------------------------------------------

local function safe_agent(opts)
  local ok, res = pcall(agent, opts)
  if ok and type(res) == "table" then return res end
  log("agent 调用失败，已降级: " .. tostring(res), "warn")
  return { status = "error", ok = false, output = {} }
end

----------------------------------------------------------------------
-- Main
----------------------------------------------------------------------

function main()
  budget(500000, 60)

  ------------------------------------------------------------
  -- Phase 1: 审计当前代码库
  ------------------------------------------------------------
  phase("audit", 1)

  local audit = safe_agent({
    prompt = [=[
你是一个资深前端技术架构师。请对 /Users/apple/dev/luft/web 项目进行完整审计。

项目目录结构：
- web/src/App.tsx — 路由配置
- web/src/main.tsx — 入口
- web/src/pages/ — 5 个页面（dashboard, runs, run-detail, workflows, backends）
- web/src/components/ — 组件（agent-card, code-editor, detail-drawer, event-stream, run-dialog, stat-card, status-badge, progress-bar, layout/）
- web/src/components/ui/ — shadcn 风格 UI 基件
- web/src/api/ — mock-client.ts, mock-data.ts, types.ts
- web/src/hooks/ — useRuns.ts, useRunDetail.ts, useDashboardStats.ts
- web/src/stores/ — ui.ts (Zustand)
- web/src/lib/ — format.ts, event-utils.ts, utils.ts
- web/src/styles/ — globals.css
- web/package.json — 依赖: React 19, TanStack Query, Zustand, react-router-dom, lucide-react, codemirror, tailwindcss v4

请做以下审计：

1. 读取 web/src/api/types.ts 了解当前类型定义
2. 读取 web/src/api/mock-client.ts 了解当前 mock API 设计
3. 读取 web/src/api/mock-data.ts 了解 mock 数据
4. 读取每个页面文件（pages/ 目录下所有）
5. 读取每个 hooks 文件
6. 读取 web/package.json

然后返回结构化审计报告，明确指出：
- 每个文件的状态（keep/modify/rewrite/delete）
- 所有 gap（mock 数据、缺少的 API、缺少的页面、错误处理、类型问题）
- 需要实现的 API endpoint 列表
- 实施计划（按优先级排序）

特别注意：
1. 当前所有数据都是 mock 的，需要设计真实 API 客户端结构
2. 缺少 WebSocket 实时通信层
3. 缺少 Report 查看功能
4. 缺少 Live 实时监控页面
5. Workflow 页面缺少编辑/保存功能
6. Backends 页面缺少 CRUD 操作
7. 品牌显示 "maestro" 需要改为 "luft"
8. 需要在基础设施层增加 loading/empty/error/retry 四态处理
    ]=],
    schema = AUDIT_SCHEMA,
    name = "audit",
    description = "审计当前 web 代码库",
    timeout_ms = 180000,
  })

  if not audit.ok then
    report({ error = "审计失败: " .. (audit.status or "unknown") })
    return
  end

  log("审计完成: " .. audit.output.summary)
  log("发现 " .. #audit.output.gaps .. " 个 gap, 计划 " .. audit.output.plan.total_files .. " 个文件需要处理")

  ------------------------------------------------------------
  -- Phase 2: 并行实现各模块
  ------------------------------------------------------------

  -- 2a: 实现 API 基础设施
  phase("implement-api", 4)

  local api_tasks = {
    {
      name = "api-types",
      detail = "重写 types.ts，增加所有真实 API 类型定义",
      prompt = [=[
重写 /Users/apple/dev/luft/web/src/api/types.ts 文件。

当前文件是 mock 类型，需要扩展为完整的真实 API 类型定义。

要求：
1. 保留所有现有类型定义（RunCheckpoint, AgentResultCache, AgentEvent, RunStatus, AgentStatus 等）
2. 增加以下新类型：

```typescript
// -- API 响应包装 --
export interface ApiResponse<T> {
  data: T
  error?: string
  meta?: {
    total: number
    page: number
    page_size: number
  }
}

// -- Workflow 管理 --
export interface Workflow {
  name: string
  path: string
  description?: string
  content?: string
  meta?: WorkflowMeta
  updated_at: string
  created_at: string
}

export interface WorkflowMeta {
  reasoning?: string
  phases?: PhaseMeta[]
}

export interface PhaseMeta {
  label: string
  description?: string
  agents?: number
  dynamic?: boolean
  depends_on?: number[]
}

// -- Backend 配置 --
export interface Backend {
  id: string
  name: string
  provider: string
  model: string
  base_url: string
  status: 'connected' | 'disconnected' | 'error'
  usage_count: number
  last_used?: string
  api_key_configured: boolean
}

export interface BackendCreateRequest {
  name: string
  provider: string
  model: string
  base_url: string
  api_key: string
}

// -- Run 发起 --
export interface RunStartRequest {
  workflow: string
  task: string
  backend: string
  extra_args?: Record<string, unknown>
}

export interface RunStartResponse {
  run_id: string
  status: RunStatus
  created_at: string
}

// -- Report --
export interface Report {
  run_id: string
  workflow_name: string
  task: string
  status: RunStatus
  report: Record<string, unknown>
  created_at: string
  total_tokens: number
  elapsed_ms: number
}

// -- Stats / Metrics --
export interface DashboardStats {
  total_runs_today: number
  active_runs: number
  success_rate: number
  total_tokens: number
  trend_tokens: number[]  // 7 day sparkline
  trend_runs: number[]    // 7 day sparkline
  trend_success: number[]  // 7 day sparkline
}

export interface BackendStats {
  backend_id: string
  backend_name: string
  total_runs: number
  total_tokens: number
  success_rate: number
  avg_elapsed_ms: number
}

// -- WebSocket 事件 --
export interface WsEvent {
  type: 'event' | 'checkpoint' | 'pong' | 'error'
  payload?: AgentEvent | RunCheckpoint | { message: string }
}

// -- Run 操作 --
export type RunAction = 'cancel' | 'retry' | 'pause'
]]

3. 导出一个枚举常量：
```typescript
export const RUN_STATUS_ORDER: RunStatus[] = ['running', 'completed', 'failed', 'cancelled']
```

4. 保持与现有代码兼容，所有已有类型名不能改变
5. 使用 export 导出所有类型

写完整的文件内容，不要省略任何部分。
      ]=],
    },
    {
      name = "api-client",
      detail = "创建真实 API 客户端",
      prompt = [=[
创建 /Users/apple/dev/luft/web/src/api/client.ts 文件。

这是一个真实的 HTTP API 客户端，替代当前 mock-client.ts。

要求：

1. 基于 fetch 实现，不引入额外 HTTP 库
2. 支持 base URL 配置（默认 /api）
3. 统一的错误处理
4. 请求/响应拦截器模式

完整实现：

```typescript
import type {
  ApiResponse,
  Backend,
  BackendCreateRequest,
  DashboardStats,
  Report,
  RunCheckpoint,
  RunStartRequest,
  RunStartResponse,
  Workflow,
  AgentEvent,
  BackendStats,
} from './types'

const BASE_URL = import.meta.env.VITE_API_BASE_URL || '/api'

class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
    public code?: string,
  ) {
    super(message)
    this.name = 'ApiError'
  }
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  signal?: AbortSignal,
): Promise<T> {
  const url = `${BASE_URL}${path}`
  const headers: Record<string, string> = {}
  if (body) {
    headers['Content-Type'] = 'application/json'
  }

  const res = await fetch(url, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
    signal,
  })

  if (!res.ok) {
    let errorMessage = `HTTP ${res.status}`
    try {
      const errBody = await res.json()
      errorMessage = errBody.error || errBody.message || errorMessage
    } catch {}
    throw new ApiError(errorMessage, res.status)
  }

  if (res.status === 204) return undefined as T
  return res.json()
}

// ---- Params types ----

interface ListRunsParams {
  status?: string
  q?: string
  page?: number
  page_size?: number
}

interface ListWorkflowsParams {
  q?: string
}

interface ListReportsParams {
  page?: number
  page_size?: number
}

interface TrendParams {
  range?: '7d' | '30d' | '90d'
}

// ---- API Client ----

export const api = {
  // Runs
  runs: {
    list: (params?: ListRunsParams) =>
      request<ApiResponse<RunCheckpoint[]>>('GET', '/runs', undefined),

    get: (runId: string) =>
      request<RunCheckpoint>('GET', `/runs/${runId}`),

    getEvents: (runId: string) =>
      request<AgentEvent[]>('GET', `/runs/${runId}/events`),

    start: (data: RunStartRequest) =>
      request<RunStartResponse>('POST', '/runs', data),

    cancel: (runId: string) =>
      request<void>('POST', `/runs/${runId}/cancel`),

    retry: (runId: string) =>
      request<void>('POST', `/runs/${runId}/retry`),

    getReport: (runId: string) =>
      request<Report>('GET', `/runs/${runId}/report`),
  },

  // Workflows
  workflows: {
    list: (params?: ListWorkflowsParams) =>
      request<ApiResponse<Workflow[]>>('GET', '/workflows'),

    get: (name: string) =>
      request<Workflow>('GET', `/workflows/${encodeURIComponent(name)}`),

    save: (name: string, content: string, meta?: Record<string, unknown>) =>
      request<Workflow>('PUT', `/workflows/${encodeURIComponent(name)}`, { content, meta }),

    delete: (name: string) =>
      request<void>('DELETE', `/workflows/${encodeURIComponent(name)}`),

    run: (name: string, task?: string, backend?: string) =>
      request<RunStartResponse>('POST', `/workflows/${encodeURIComponent(name)}/run`, { task, backend }),
  },

  // Backends
  backends: {
    list: () =>
      request<Backend[]>('GET', '/backends'),

    create: (data: BackendCreateRequest) =>
      request<Backend>('POST', '/backends', data),

    update: (id: string, data: Partial<BackendCreateRequest>) =>
      request<Backend>('PATCH', `/backends/${id}`, data),

    delete: (id: string) =>
      request<void>('DELETE', `/backends/${id}`),

    test: (id: string) =>
      request<{ ok: boolean; message: string }>('POST', `/backends/${id}/test`),
  },

  // Stats
  stats: {
    dashboard: () =>
      request<DashboardStats>('GET', '/stats/dashboard'),

    trends: (params?: TrendParams) =>
      request<{
        tokens: { date: string; value: number }[]
        runs: { date: string; value: number }[]
        success_rate: { date: string; value: number }[]
      }>('GET', '/stats/trends'),

    backends: () =>
      request<BackendStats[]>('GET', '/stats/backends'),
  },

  // Reports
  reports: {
    list: (params?: ListReportsParams) =>
      request<ApiResponse<Report[]>>('GET', '/reports'),

    get: (runId: string) =>
      request<Report>('GET', `/reports/${runId}`),
  },
}

export { ApiError }
export type { ListRunsParams, ListWorkflowsParams, TrendParams }
```

注意：
1. 使用 `import.meta.env.VITE_API_BASE_URL` 作为可配置的 base URL
2. 所有请求都经过统一的 request 函数，统一处理错误
3. 导出 ApiError 类供外部使用
4. 所有方法都是类型安全的

写完整的文件内容。
      ]=],
    },
    {
      name = "ws-client",
      detail = "创建 WebSocket 客户端",
      prompt = [=[
创建 /Users/apple/dev/luft/web/src/api/ws.ts 文件。

这是一个 WebSocket 客户端，用于实时接收 Run 事件流。

完整实现：

```typescript
import type { AgentEvent, RunCheckpoint, WsEvent } from './types'

type WsCallback = {
  onEvent?: (event: AgentEvent) => void
  onCheckpoint?: (checkpoint: RunCheckpoint) => void
  onError?: (error: string) => void
  onStatusChange?: (status: 'connecting' | 'connected' | 'disconnected' | 'error') => void
}

const WS_BASE_URL = import.meta.env.VITE_WS_BASE_URL || (() => {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${window.location.host}/ws`
})()

class WsClient {
  private ws: WebSocket | null = null
  private runId: string | null = null
  private callbacks: WsCallback = {}
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private pingTimer: ReturnType<typeof setInterval> | null = null
  private maxRetries = 5
  private retryCount = 0
  private retryDelay = 1000

  connect(runId: string, callbacks: WsCallback) {
    this.disconnect()
    this.runId = runId
    this.callbacks = callbacks
    this.retryCount = 0
    this._connect()
  }

  private _connect() {
    if (!this.runId) return
    this.callbacks.onStatusChange?.('connecting')

    try {
      this.ws = new WebSocket(`${WS_BASE_URL}/runs/${this.runId}`)
    } catch (e) {
      this.callbacks.onStatusChange?.('error')
      this._scheduleReconnect()
      return
    }

    this.ws.onopen = () => {
      this.retryCount = 0
      this.retryDelay = 1000
      this.callbacks.onStatusChange?.('connected')
      this._startPing()
    }

    this.ws.onmessage = (msg) => {
      try {
        const data: WsEvent = JSON.parse(msg.data)
        switch (data.type) {
          case 'event':
            this.callbacks.onEvent?.(data.payload as AgentEvent)
            break
          case 'checkpoint':
            this.callbacks.onCheckpoint?.(data.payload as RunCheckpoint)
            break
          case 'pong':
            break
          case 'error':
            this.callbacks.onError?.((data.payload as { message: string }).message)
            break
        }
      } catch (e) {
        console.warn('[ws] parse error:', e)
      }
    }

    this.ws.onclose = () => {
      this.callbacks.onStatusChange?.('disconnected')
      this._stopPing()
      this._scheduleReconnect()
    }

    this.ws.onerror = () => {
      this.callbacks.onStatusChange?.('error')
    }
  }

  private _scheduleReconnect() {
    if (this.retryCount >= this.maxRetries) return
    this.reconnectTimer = setTimeout(() => {
      this.retryCount++
      this.retryDelay = Math.min(this.retryDelay * 2, 30000)
      this._connect()
    }, this.retryDelay)
  }

  private _startPing() {
    this.pingTimer = setInterval(() => {
      if (this.ws?.readyState === WebSocket.OPEN) {
        this.ws.send(JSON.stringify({ type: 'ping' }))
      }
    }, 30000)
  }

  private _stopPing() {
    if (this.pingTimer) {
      clearInterval(this.pingTimer)
      this.pingTimer = null
    }
  }

  disconnect() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    this._stopPing()
    if (this.ws) {
      this.ws.onclose = null
      this.ws.close()
      this.ws = null
    }
    this.runId = null
    this.callbacks = {}
  }

  isConnected() {
    return this.ws?.readyState === WebSocket.OPEN
  }
}

export const wsClient = new WsClient()
export type { WsCallback }
```

注意：
1. 自动重连（指数退避，最多 5 次）
2. 30 秒心跳 ping
3. 连接状态回调
4. 整洁的断开清理

写完整的文件内容。
      ]=],
    },
    {
      name = "hooks-live",
      detail = "创建实时 hooks",
      prompt = [=[
创建 /Users/apple/dev/luft/web/src/hooks/useLiveRun.ts 文件。

这是实时 WebSocket 驱动的 Hook，用于在 Run Detail 页面接收实时事件。

完整实现：

```typescript
import { useEffect, useState, useCallback, useRef } from 'react'
import { wsClient } from '@/api/ws'
import type { WsCallback } from '@/api/ws'
import type { AgentEvent, RunCheckpoint } from '@/api/types'

interface LiveRunState {
  events: AgentEvent[]
  checkpoint: RunCheckpoint | null
  connectionStatus: 'connecting' | 'connected' | 'disconnected' | 'error'
  paused: boolean
}

export function useLiveRun(runId: string | undefined) {
  const [state, setState] = useState<LiveRunState>({
    events: [],
    checkpoint: null,
    connectionStatus: 'disconnected',
    paused: false,
  })
  const eventsRef = useRef<AgentEvent[]>([])
  const pausedRef = useRef(false)

  const onEvent = useCallback((event: AgentEvent) => {
    if (pausedRef.current) return
    eventsRef.current = [...eventsRef.current, event]
    setState((prev) => ({
      ...prev,
      events: eventsRef.current,
    }))
  }, [])

  const onCheckpoint = useCallback((checkpoint: RunCheckpoint) => {
    setState((prev) => ({
      ...prev,
      checkpoint,
    }))
  }, [])

  const onStatusChange = useCallback((connectionStatus: LiveRunState['connectionStatus']) => {
    setState((prev) => ({ ...prev, connectionStatus }))
  }, [])

  useEffect(() => {
    if (!runId) return

    eventsRef.current = []
    pausedRef.current = false

    const callbacks: WsCallback = {
      onEvent,
      onCheckpoint,
      onStatusChange,
    }

    wsClient.connect(runId, callbacks)

    return () => {
      wsClient.disconnect()
    }
  }, [runId, onEvent, onCheckpoint, onStatusChange])

  const togglePause = useCallback(() => {
    pausedRef.current = !pausedRef.current
    setState((prev) => ({ ...prev, paused: pausedRef.current }))
  }, [])

  const clearEvents = useCallback(() => {
    eventsRef.current = []
    setState((prev) => ({ ...prev, events: [] }))
  }, [])

  return {
    ...state,
    togglePause,
    clearEvents,
  }
}
```

写完整的文件内容。
      ]=],
    },
  }

  local api_results = parallel(api_tasks, function(task)
    return {
      prompt = [=[
你是资深前端工程师。任务：实现 Web Dashboard 的 API 基础设施模块。

你的任务组: ]=] .. task.name .. [=[

].. task.detail .. [=[

读取当前相关文件以了解现有代码风格，然后创建/修改文件。

完成后返回 JSON：{
  "task": "任务名称",
  "files_created": ["path1", "path2"],
  "files_modified": [],
  "total_lines": <数字>,
  "summary": "完成摘要",
  "errors": []
}
      ]=],
      timeout_ms = 180000,
    }
  end)

  -- 收集 API 阶段结果
  local api_files_created = {}
  local api_errors = {}
  for _, r in ipairs(api_results) do
    if r.ok and r.output then
      if r.output.files_created then
        for _, f in ipairs(r.output.files_created) do
          table.insert(api_files_created, f)
        end
      end
      if r.output.errors then
        for _, e in ipairs(r.output.errors) do
          table.insert(api_errors, e)
        end
      end
      log("API 模块 " .. (r.output.task or "?") .. ": " .. (r.output.summary or "done"))
    else
      log("API 模块失败: " .. (r.status or "unknown"), "warn")
    end
  end
  log("API 阶段完成: 创建 " .. #api_files_created .. " 个文件, " .. #api_errors .. " 个错误")

  ------------------------------------------------------------
  -- Phase 2b: 实现页面
  ------------------------------------------------------------
  phase("implement-pages", 4)

  local page_tasks = {
    {
      name = "page-dashboard",
      detail = "重写 Dashboard 页面",
      prompt = [=[
重写 /Users/apple/dev/luft/web/src/pages/dashboard.tsx 文件。

当前文件需要从 mock 切换到真实 API，并增加统计卡片和趋势图。

要求：
1. 使用 useQuery 从真实 API 获取数据
2. 保留 StatCard 和 ProgressBar 组件
3. 增加活跃 Runs 列表（实时更新）
4. 增加 7 天趋势 sparkline
5. 增加快速操作入口

设计：
- 顶部 4 个统计卡片（今日 Runs、活跃中、成功率、Token 消耗）
- 中间两栏：活跃 Runs / 最近完成
- 底部 7 天趋势（Token 和 Run 数）

```tsx
import { useQuery } from '@tanstack/react-query'
import { api } from '@/api/client'
import { StatCard } from '@/components/stat-card'
import { ProgressBar } from '@/components/progress-bar'
import { StatusBadge } from '@/components/status-badge'
import { Card } from '@/components/ui/card'
import { formatTokens, formatRelativeTime } from '@/lib/format'
import { Activity, ListChecks, TrendingUp, Zap } from 'lucide-react'
import { Link } from 'react-router-dom'
```

注意：
- 所有数据从 api.stats.dashboard() 获取
- 活跃 Runs 列表从 api.runs.list({ status: 'running' }) 获取
- 显示 loading/empty/error 三态
- 保持深色主题风格一致

写完整的文件内容。
      ]=],
    },
    {
      name = "page-live",
      detail = "创建 Live 实时监控页面",
      prompt = [=[
创建 /Users/apple/dev/luft/web/src/pages/live.tsx 文件。

这是新增的实时监控页面，展示所有正在运行的 Workflow。

设计：
```tsx
import { useQuery } from '@tanstack/react-query'
import { api } from '@/api/client'
import { Card } from '@/components/ui/card'
import { StatusBadge } from '@/components/status-badge'
import { ProgressBar } from '@/components/progress-bar'
import { Button } from '@/components/ui/button'
import { formatElapsed, formatTokens } from '@/lib/format'
import { RefreshCw, Pause, Play } from 'lucide-react'
import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
```

布局：
- 顶部标题 + 刷新按钮 + 自动刷新开关
- 大卡片网格，每个卡片展示一个运行中的 Run
- 每个卡片包含：
  - 任务名称 + Run ID 前缀
  - 当前 Phase + Agent 进度
  - ProgressBar
  - Token 消耗 + 运行时间
  - 状态指示器
- 点击卡片跳转到 Run 详情

数据来源：
- api.runs.list({ status: 'running' }) 每 5 秒自动刷新
- 或者使用 WebSocket 获取实时更新

需要处理：
- 空状态（没有运行中的 Run）
- 自动刷新（轮询间隔 5 秒）
- 手动暂停/恢复刷新

写完整的文件内容。
      ]=],
    },
    {
      name = "page-reports",
      detail = "创建 Reports 页面",
      prompt = [=[
创建 /Users/apple/dev/luft/web/src/pages/reports.tsx 文件。

这是新增的 Report 汇总页面，展示所有 Run 的最终 report() 输出。

设计：
```tsx
import { useQuery } from '@tanstack/react-query'
import { api } from '@/api/client'
import { Card } from '@/components/ui/card'
import { StatusBadge } from '@/components/status-badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { formatTokens, formatRelativeTime } from '@/lib/format'
import { FileText, Search, Star, ExternalLink } from 'lucide-react'
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
```

布局：
- 顶部搜索框 + 过滤
- 报告列表卡片
- 每个卡片显示：
  - 任务名称 + Run ID
  - 状态 + 时间
  - Token 消耗
  - 报告摘要（前 2 行）
  - 收藏按钮
- 点击卡片展开报告详情或跳转

数据来源：
- api.reports.list()

写完整的文件内容。
      ]=],
    },
    {
      name = "page-metrics",
      detail = "创建 Metrics 页面",
      prompt = [=[
创建 /Users/apple/dev/luft/web/src/pages/metrics.tsx 文件。

这是新增的指标分析页面，展示 Token 消耗、执行时间、成功率趋势。

设计：
```tsx
import { useQuery } from '@tanstack/react-query'
import { api } from '@/api/client'
import { Card } from '@/components/ui/card'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select'
import { formatTokens } from '@/lib/format'
import { useState } from 'react'
```

布局：
- 顶部时间范围选择器（7d / 30d / 90d）
- 4 个趋势卡片：
  - Token 消耗趋势（柱状图或折线图，用纯 CSS 实现 sparkline）
  - 每日 Run 数趋势
  - 成功率趋势
  - 平均执行时间趋势
- Backend 对比表格：
  - 各 Backend 的 Run 数、Token 消耗、成功率、平均耗时
  - 使用 Table 组件

数据来源：
- api.stats.trends({ range })
- api.stats.backends()

注意：图表用纯 CSS/SVG 实现，不需要引入 chart 库。可以用 div 条来实现简单的柱状图。

写完整的文件内容。
      ]=],
    },
  }

  local page_results = parallel(page_tasks, function(task)
    return {
      prompt = [=[
你是资深前端工程师。任务：实现 Web Dashboard 的页面模块。

你的任务组: ]=] .. task.name .. [=[

].. task.detail .. [=[

读取当前相关文件以了解现有代码风格，然后创建/修改文件。

完成后返回 JSON：{
  "task": "任务名称",
  "files_created": ["path1"],
  "files_modified": ["path2"],
  "total_lines": <数字>,
  "summary": "完成摘要",
  "errors": []
}
      ]=],
      timeout_ms = 180000,
    }
  end)

  -- 收集页面阶段结果
  local page_files_created = {}
  for _, r in ipairs(page_results) do
    if r.ok and r.output then
      if r.output.files_created then
        for _, f in ipairs(r.output.files_created) do
          table.insert(page_files_created, f)
        end
      end
      log("页面模块 " .. (r.output.task or "?") .. ": " .. (r.output.summary or "done"))
    else
      log("页面模块失败: " .. (r.status or "unknown"), "warn")
    end
  end

  ------------------------------------------------------------
  -- Phase 2c: 实现组件及基础设施
  ------------------------------------------------------------
  phase("implement-components", 3)

  local component_tasks = {
    {
      name = "nav-routing",
      detail = "重写 App.tsx 路由和导航",
      prompt = [=[
重写 /Users/apple/dev/luft/web/src/App.tsx 文件。

同时重写 /Users/apple/dev/luft/web/src/components/layout/top-nav.tsx 文件。

App.tsx 需要：
1. 增加新路由：/live, /reports, /metrics
2. 保持原有路由不变
3. 使用 React.lazy 实现按需加载

top-nav.tsx 需要：
1. 品牌名从 "maestro" 改为 "luft"
2. 增加新导航项：Live, Reports, Metrics
3. 保持原有设计风格

App.tsx 完整内容：
```tsx
import { Routes, Route } from 'react-router-dom'
import { TopNav } from '@/components/layout/top-nav'
import { RunDialog } from '@/components/run-dialog'
import { lazy, Suspense } from 'react'

const Dashboard = lazy(() => import('@/pages/dashboard'))
const Runs = lazy(() => import('@/pages/runs'))
const RunDetail = lazy(() => import('@/pages/run-detail'))
const Workflows = lazy(() => import('@/pages/workflows'))
const Backends = lazy(() => import('@/pages/backends'))
const Live = lazy(() => import('@/pages/live'))
const Reports = lazy(() => import('@/pages/reports'))
const Metrics = lazy(() => import('@/pages/metrics'))

function LoadingFallback() {
  return (
    <div className="flex items-center justify-center h-64">
      <div className="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent" />
    </div>
  )
}

export default function App() {
  return (
    <div className="min-h-screen bg-bg">
      <TopNav />
      <main className="mx-auto max-w-7xl px-6 py-6">
        <Suspense fallback={<LoadingFallback />}>
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/live" element={<Live />} />
            <Route path="/runs" element={<Runs />} />
            <Route path="/runs/:runId" element={<RunDetail />} />
            <Route path="/workflows" element={<Workflows />} />
            <Route path="/workflows/:workflowName" element={<Workflows />} />
            <Route path="/backends" element={<Backends />} />
            <Route path="/reports" element={<Reports />} />
            <Route path="/reports/:runId" element={<Reports />} />
            <Route path="/metrics" element={<Metrics />} />
          </Routes>
        </Suspense>
      </main>
      <RunDialog />
    </div>
  )
}
```

top-nav.tsx 完整内容（保持原风格，改品牌名和路由）：
```tsx
import { NavLink } from 'react-router-dom'
import { Activity, ListChecks, FileCode2, Server, Play, Radio, FileText, BarChart3 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useUIStore } from '@/stores/ui'
import { cn } from '@/lib/utils'

const navItems = [
  { to: '/', label: 'Dashboard', icon: Activity },
  { to: '/live', label: 'Live', icon: Radio },
  { to: '/runs', label: 'Runs', icon: ListChecks },
  { to: '/workflows', label: 'Workflows', icon: FileCode2 },
  { to: '/backends', label: 'Backends', icon: Server },
  { to: '/reports', label: 'Reports', icon: FileText },
  { to: '/metrics', label: 'Metrics', icon: BarChart3 },
]

export function TopNav() {
  const setRunDialogOpen = useUIStore((s) => s.setRunDialogOpen)

  return (
    <header className="sticky top-0 z-40 flex h-14 items-center justify-between border-b border-border bg-bg-surface/80 px-6 backdrop-blur-md">
      <div className="flex items-center gap-2">
        <div className="flex items-center gap-2 mr-8">
          <div className="flex h-7 w-7 items-center justify-center rounded-md bg-primary/15">
            <span className="text-primary text-base font-bold font-display">L</span>
          </div>
          <span className="text-base font-semibold font-display tracking-tight">luft</span>
        </div>
        <nav className="flex items-center gap-1">
          {navItems.map(({ to, label, icon: Icon }) => (
            <NavLink
              key={to}
              to={to}
              end={to === '/'}
              className={({ isActive }) =>
                cn(
                  'flex items-center gap-2 rounded-md px-3 py-1.5 text-sm font-medium transition-colors',
                  isActive
                    ? 'bg-hover text-primary'
                    : 'text-muted-foreground hover:text-foreground hover:bg-hover/50'
                )
              }
            >
              <Icon className="h-4 w-4" />
              {label}
            </NavLink>
          ))}
        </nav>
      </div>
      <Button size="sm" onClick={() => setRunDialogOpen(true)}>
        <Play className="h-3.5 w-3.5" />
        Run
      </Button>
    </header>
  )
}
```

写完整的两个文件内容。
      ]=],
    },
    {
      name = "run-detail-enhance",
      detail = "增强 Run Detail 页面",
      prompt = [=[
重写 /Users/apple/dev/luft/web/src/pages/run-detail.tsx 文件。

需要增加：
1. Report 面板（展示 markdown 或 JSON report）
2. 重试/取消按钮
3. WebSocket 实时事件流
4. 增强的 Agent Drawer（显示 AcpRequest 原始日志）

设计：
```tsx
import { useParams } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { api } from '@/api/client'
import { useLiveRun } from '@/hooks/useLiveRun'
import { useRunDetail } from '@/hooks/useRunDetail'
import { Card } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { StatusBadge } from '@/components/status-badge'
import { ProgressBar } from '@/components/progress-bar'
import { AgentCard } from '@/components/agent-card'
import { DetailDrawer } from '@/components/detail-drawer'
import { EventStream } from '@/components/event-stream'
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs'
import { formatElapsed, formatTokens } from '@/lib/format'
import { ArrowLeft, RotateCcw, XCircle, Terminal, FileText } from 'lucide-react'
import { useState, useMemo } from 'react'
import { useNavigate } from 'react-router-dom'
import { toast } from 'sonner'
```

布局：
- 顶部：返回按钮 + 标题 + 状态 + 操作按钮（重试、取消）
- 中间 Tabs：Timeline（当前进度）/ Report（最终输出）/ Logs（事件流）
- Timeline Tab：左栏 Phase 列表 + 右栏 Agent 卡片网格
- Report Tab：JSON 或 Markdown 渲染
- Logs Tab：全屏事件流

重试/取消实现：
- 调用 api.runs.cancel(runId) 或 api.runs.retry(runId)
- toast 提示结果
- 刷新数据

Report 面板：
- 从 api.runs.getReport(runId) 获取
- JSON 渲染为格式化 JSON 或 Markdown

写完整的文件内容。
      ]=],
    },
    {
      name = "workflows-enhance",
      detail = "增强 Workflows 页面",
      prompt = [=[
重写 /Users/apple/dev/luft/web/src/pages/workflows.tsx 文件。

需要增加：
1. 文件浏览器（列出所有 workflow 文件）
2. Lua 代码编辑器（只读 + 可编辑模式）
3. Schema 预览面板
4. 保存/运行按钮

设计：
```tsx
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import { Card } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { CodeEditor } from '@/components/code-editor'
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs'
import { Badge } from '@/components/ui/badge'
import { Separator } from '@/components/ui/separator'
import { FileCode2, Save, Play, Plus, FileText, Trash2 } from 'lucide-react'
import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { toast } from 'sonner'
```

布局：
- 左侧：文件浏览器列表（20% 宽度）
- 右侧：代码编辑器 + 底部 Schema 预览面板
- 顶部工具栏：保存 / 运行 / 删除按钮
- 编辑模式下显示保存按钮，只读模式下隐藏

文件浏览器：
- 列出所有 workflow 文件
- 当前选中文件高亮
- 新建文件按钮

Schema 预览：
- 从 Lua meta 中提取 phase 信息
- 显示为结构化列表或流程图

写完整的文件内容。
      ]=],
    },
  }

  local component_results = parallel(component_tasks, function(task)
    return {
      prompt = [=[
你是资深前端工程师。任务：实现 Web Dashboard 的组件和基础设施。

你的任务组: ]=] .. task.name .. [=[

].. task.detail .. [=[

读取当前相关文件以了解现有代码风格，然后创建/修改文件。

完成后返回 JSON：{
  "task": "任务名称",
  "files_created": ["path1"],
  "files_modified": ["path2"],
  "total_lines": <数字>,
  "summary": "完成摘要",
  "errors": []
}
      ]=],
      timeout_ms = 180000,
    }
  end)

  ------------------------------------------------------------
  -- Phase 3: 验证构建
  ------------------------------------------------------------
  phase("verify", 1)

  log("开始验证 TypeScript 构建", "info")

  local verify = safe_agent({
    prompt = [=[
你是一个前端构建工程师。需要验证 Luft Web Dashboard 能否通过 TypeScript 编译。

项目路径: /Users/apple/dev/luft/web

任务：
1. 运行 `npx tsc --noEmit 2>&1` 检查 TypeScript 编译错误
2. 如果有错误，读取相关文件并修复
3. 重复直到编译通过或达到 5 次尝试
4. 运行 `npx vite build 2>&1` 检查构建是否成功

特别注意：
- 新创建的文件（live.tsx, reports.tsx, metrics.tsx, client.ts, ws.ts, useLiveRun.ts）可能有导入错误
- 修改过的文件（dashboard.tsx, run-detail.tsx, workflows.tsx, App.tsx, top-nav.tsx）可能需要调整
- 确保所有 import 路径正确（使用 @/ 别名）
- 确保所有类型定义匹配

对于每个错误：
1. 读取错误文件
2. 理解错误原因
3. 修复（使用 Edit 工具）
4. 重新运行 tsc

返回 JSON：
{
  "success": true/false,
  "output": "最终构建输出",
  "errors": ["错误列表"],
  "fixes_applied": ["已修复的问题列表"]
}
    ]=],
    schema = VERIFY_RESULT,
    name = "verify",
    description = "TypeScript 编译验证",
    timeout_ms = 300000,
  })

  ------------------------------------------------------------
  -- 最终报告
  ------------------------------------------------------------

  local all_files = {}
  local all_errors = {}

  -- 合并 API 阶段结果
  for _, r in ipairs(api_results) do
    if r.ok and r.output then
      if r.output.files_created then
        for _, f in ipairs(r.output.files_created) do
          table.insert(all_files, { path = f, action = "created", phase = "api" })
        end
      end
      if r.output.errors then
        for _, e in ipairs(r.output.errors) do
          table.insert(all_errors, { phase = "api", error = e })
        end
      end
    end
  end

  -- 合并页面阶段结果
  for _, r in ipairs(page_results) do
    if r.ok and r.output then
      if r.output.files_created then
        for _, f in ipairs(r.output.files_created) do
          table.insert(all_files, { path = f, action = "created", phase = "pages" })
        end
      end
      if r.output.files_modified then
        for _, f in ipairs(r.output.files_modified) do
          table.insert(all_files, { path = f, action = "modified", phase = "pages" })
        end
      end
      if r.output.errors then
        for _, e in ipairs(r.output.errors) do
          table.insert(all_errors, { phase = "pages", error = e })
        end
      end
    end
  end

  -- 合并组件阶段结果
  for _, r in ipairs(component_results) do
    if r.ok and r.output then
      if r.output.files_created then
        for _, f in ipairs(r.output.files_created) do
          table.insert(all_files, { path = f, action = "created", phase = "components" })
        end
      end
      if r.output.files_modified then
        for _, f in ipairs(r.output.files_modified) do
          table.insert(all_files, { path = f, action = "modified", phase = "components" })
        end
      end
      if r.output.errors then
        for _, e in ipairs(r.output.errors) do
          table.insert(all_errors, { phase = "components", error = e })
        end
      end
    end
  end

  local build_passed = verify.ok and verify.output and verify.output.success
  local fix_count = 0
  if verify.ok and verify.output and verify.output.fixes_applied then
    fix_count = #verify.output.fixes_applied
  end

  report({
    workflow = "web-redesign",
    project = "luft-web",
    phases = {
      {
        name = "audit",
        status = audit.ok and "completed" or "failed",
        gaps_found = audit.ok and #audit.output.gaps or 0,
      },
      {
        name = "implement-api",
        status = "completed",
        files = #api_files_created,
      },
      {
        name = "implement-pages",
        status = "completed",
        files = #page_files_created,
      },
      {
        name = "implement-components",
        status = "completed",
      },
      {
        name = "verify",
        status = build_passed and "completed" or "needs_fixes",
        fix_count = fix_count,
      },
    },
    files_changed = all_files,
    total_files = #all_files,
    errors = all_errors,
    build_status = build_passed and "passed" or "failed",
    build_output = verify.ok and verify.output and verify.output.output or nil,
    summary = build_passed
      and "Web 重设计完成！所有文件已创建/更新，TypeScript 编译通过。"
      or "Web 重设计大部分完成，但构建有错误需要手动修复。",
  })
end