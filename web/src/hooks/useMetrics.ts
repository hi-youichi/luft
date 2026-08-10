import { useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { api, queryKeys } from '@/api'
import { useDashboardStats } from '@/hooks/useDashboardStats'
import type { RunSummary, RunStatus } from '@/api/types'

const STATUS_COLORS: Record<RunStatus, string> = {
  running: '#3b82f6',
  completed: '#00e676',
  failed: '#ef4444',
  cancelled: '#f59e0b',
}

const STATUS_LABELS: Record<RunStatus, string> = {
  running: '运行中',
  completed: '已完成',
  failed: '失败',
  cancelled: '已取消',
}

export interface DailyMetric {
  date: string
  label: string
  runs: number
  tokens: number
}

export interface MetricsData {
  totalRuns: number
  successRate: number
  totalTokens: number
  avgDurationMs: number
  statusBreakdown: { label: string; value: number; color: string }[]
  dailyMetrics: DailyMetric[]
  topRunsByTokens: RunSummary[]
  activeCount: number
}

function useAllRuns(timeRange: 'today' | '24h' | '7d' | 'all' = '7d') {
  return useQuery({
    queryKey: [...queryKeys.runs.list({ status: 'all', time: timeRange }), 'metrics'],
    queryFn: () => api.runs.list({ status: 'all', time: timeRange }),
  })
}

export function useMetrics(timeRange: 'today' | '24h' | '7d' | 'all' = '7d') {
  const { data: stats, isLoading: statsLoading } = useDashboardStats()
  const { data: runsData, isLoading: runsLoading } = useAllRuns(timeRange)

  const metrics = useMemo<MetricsData | null>(() => {
    const runs: RunSummary[] =
      runsData && 'runs' in runsData
        ? runsData.runs
        : Array.isArray(runsData)
          ? runsData
          : []

    const totalRuns = runs.length
    const completed = runs.filter((r) => r.status === 'completed')
    const failed = runs.filter((r) => r.status === 'failed')
    const successRate = completed.length + failed.length > 0
      ? (completed.length / (completed.length + failed.length)) * 100
      : 0

    const totalTokens = runs.reduce((sum, r) => sum + r.total_tokens, 0)
    const finishedRuns = runs.filter((r) => r.status !== 'running')
    const avgDurationMs = finishedRuns.length > 0
      ? finishedRuns.reduce((sum, r) => sum + r.elapsed_ms, 0) / finishedRuns.length
      : 0

    const statusCounts: Record<RunStatus, number> = {
      running: 0,
      completed: 0,
      failed: 0,
      cancelled: 0,
    }
    for (const run of runs) {
      statusCounts[run.status]++
    }

    const statusBreakdown = (Object.keys(statusCounts) as RunStatus[])
      .filter((s) => statusCounts[s] > 0)
      .map((s) => ({
        label: STATUS_LABELS[s],
        value: statusCounts[s],
        color: STATUS_COLORS[s],
      }))

    const dailyMap = new Map<string, { runs: number; tokens: number }>()
    const now = new Date()
    const days = timeRange === 'today' ? 1 : timeRange === '24h' ? 1 : timeRange === '7d' ? 7 : 30
    for (let i = days - 1; i >= 0; i--) {
      const d = new Date(now)
      d.setDate(d.getDate() - i)
      const key = d.toISOString().slice(0, 10)
      dailyMap.set(key, { runs: 0, tokens: 0 })
    }

    for (const run of runs) {
      const dayKey = run.started_at.slice(0, 10)
      if (dailyMap.has(dayKey)) {
        const entry = dailyMap.get(dayKey)!
        entry.runs++
        entry.tokens += run.total_tokens
      }
    }

    const dailyMetrics: DailyMetric[] = Array.from(dailyMap.entries()).map(([date, val]) => {
      const d = new Date(date)
      const label = days <= 7
        ? `${d.getMonth() + 1}/${d.getDate()}`
        : `${d.getDate()}`
      return {
        date,
        label,
        runs: val.runs,
        tokens: val.tokens,
      }
    })

    const topRunsByTokens = [...runs]
      .sort((a, b) => b.total_tokens - a.total_tokens)
      .slice(0, 8)

    return {
      totalRuns,
      successRate,
      totalTokens,
      avgDurationMs,
      statusBreakdown,
      dailyMetrics,
      topRunsByTokens,
      activeCount: statusCounts.running,
    }
  }, [runsData, timeRange])

  return {
    data: metrics,
    isLoading: statsLoading || runsLoading,
    stats,
  }
}
