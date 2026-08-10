import {
  Activity,
  ListChecks,
  FileCode2,
  Server,
  Play,
  RadioTower,
  BarChart3,
  FileBarChart,
  type LucideIcon,
} from 'lucide-react'

export interface RouteConfig {
  path: string
  label: string
  description: string
  icon: LucideIcon
  keywords: string[]
  showInNav: boolean
  end?: boolean
  parent?: RouteKey
  crumbLabel?: (params: Record<string, string | undefined>) => string
}

export type RouteKey =
  | 'dashboard'
  | 'runs'
  | 'runDetail'
  | 'workflows'
  | 'workflowDetail'
  | 'backends'
  | 'live'
  | 'metrics'
  | 'reports'

export const routes = {
  dashboard: {
    path: '/',
    label: 'Dashboard',
    description: '概览活跃 Runs 与今日统计',
    icon: Activity,
    keywords: ['home', 'overview', 'stats', '概览', '首页'],
    showInNav: true,
    end: true,
    crumbLabel: () => 'Dashboard',
  },
  runs: {
    path: '/runs',
    label: 'Runs',
    description: '浏览所有执行记录',
    icon: ListChecks,
    keywords: ['executions', 'tasks', '运行', '执行'],
    showInNav: true,
    crumbLabel: () => 'Runs',
  },
  runDetail: {
    path: '/runs/:runId',
    label: 'Run Detail',
    description: '查看单个 Run 的详细信息',
    icon: ListChecks,
    keywords: ['execution', 'detail', 'log', '详情'],
    showInNav: false,
    parent: 'runs',
    crumbLabel: (p) => `#${p?.runId ?? ''}`,
  },
  workflows: {
    path: '/workflows',
    label: 'Workflows',
    description: '编辑与管理 Lua 工作流',
    icon: FileCode2,
    keywords: ['scripts', 'lua', 'code', '工作流', '脚本'],
    showInNav: true,
    crumbLabel: () => 'Workflows',
  },
  workflowDetail: {
    path: '/workflows/:workflowName',
    label: 'Workflow Detail',
    description: '编辑工作流脚本',
    icon: FileCode2,
    keywords: ['editor', 'lua', 'script', '编辑'],
    showInNav: false,
    parent: 'workflows',
    crumbLabel: (p) => p?.workflowName ?? 'Workflow',
  },
  backends: {
    path: '/backends',
    label: 'Backends',
    description: '配置 LLM 后端连接',
    icon: Server,
    keywords: ['providers', 'models', 'llm', '后端', '模型'],
    showInNav: true,
    crumbLabel: () => 'Backends',
  },
  live: {
    path: '/live',
    label: 'Live',
    description: '实时监控运行状态',
    icon: RadioTower,
    keywords: ['monitor', 'stream', 'realtime', '实时', '监控'],
    showInNav: true,
    crumbLabel: () => 'Live',
  },
  metrics: {
    path: '/metrics',
    label: 'Metrics',
    description: '查看吞吐量与延迟指标',
    icon: BarChart3,
    keywords: ['charts', 'analytics', 'stats', '指标', '分析'],
    showInNav: true,
    crumbLabel: () => 'Metrics',
  },
  reports: {
    path: '/reports',
    label: 'Reports',
    description: '汇总查看所有运行的 Findings 与报告',
    icon: FileBarChart,
    keywords: ['findings', 'severity', 'audit', '报告', '发现'],
    showInNav: true,
    crumbLabel: () => 'Reports',
  },
} satisfies Record<RouteKey, RouteConfig>

export const navItems = (Object.values(routes) as RouteConfig[]).filter((r) => r.showInNav)

export const runActionIcon = Play

export function searchRoutes(query: string): RouteConfig[] {
  const q = query.toLowerCase().trim()
  if (!q) return Object.values(routes).filter((r) => r.showInNav)
  return Object.values(routes).filter(
    (r) =>
      r.showInNav &&
      (r.label.toLowerCase().includes(q) ||
        r.description.toLowerCase().includes(q) ||
        r.keywords.some((k) => k.toLowerCase().includes(q))),
  )
}

function replaceParams(pattern: string, params: Record<string, string | undefined>): string {
  let result = pattern
  for (const [key, value] of Object.entries(params)) {
    if (value != null) {
      result = result.replace(`:${key}`, value)
    }
  }
  return result
}

interface Crumb {
  label: string
  to: string
}

export function buildBreadcrumbs(
  routeKey: RouteKey,
  params: Record<string, string | undefined>,
): Crumb[] {
  const route: RouteConfig = routes[routeKey]
  const crumbs: Crumb[] = []

  if (route.parent) {
    crumbs.push(...buildBreadcrumbs(route.parent, params))
  }

  crumbs.push({
    label: route.crumbLabel ? route.crumbLabel(params) : route.label,
    to: replaceParams(route.path, params),
  })

  return crumbs
}
