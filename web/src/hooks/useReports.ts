import { useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { api, queryKeys } from '@/api'
import type { RunSummary, RunCheckpoint } from '@/api/types'

export type FindingSeverity = 'critical' | 'high' | 'medium' | 'low'

export interface ReportItem {
  id: string
  run_id: string
  task: string
  severity: FindingSeverity
  message: string
  source?: string
  run_status: RunSummary['status']
  started_at: string
  elapsed_ms: number
  total_tokens: number
}

export interface ArtifactSummary {
  run_id: string
  task: string
  name: string
  path: string
  size: number
  mime_type: string
  created_at: string
}

export interface ReportsData {
  totalFindings: number
  severityCounts: Record<FindingSeverity, number>
  reports: ReportItem[]
  artifacts: ArtifactSummary[]
  runsWithFindings: number
  runsCompleted: number
  runsFailed: number
}

const SEVERITY_ORDER: FindingSeverity[] = ['critical', 'high', 'medium', 'low']

type TimeRange = 'today' | '24h' | '7d' | 'all'

function useReportRuns(timeRange: TimeRange) {
  return useQuery({
    queryKey: [...queryKeys.runs.list({ status: 'all', time: timeRange }), 'reports'],
    queryFn: () => api.runs.list({ status: 'all', time: timeRange }),
  })
}

function useReportCheckpoints(runs: RunSummary[]) {
  const finishedRuns = runs.filter((r) => r.status === 'completed' || r.status === 'failed')
  return useQuery({
    queryKey: ['reports', 'checkpoints', finishedRuns.map((r) => r.run_id)],
    queryFn: async () => {
      const results = await Promise.all(
        finishedRuns.map((r) => api.runs.get(r.run_id).catch(() => null)),
      )
      const map = new Map<string, RunCheckpoint>()
      results.forEach((cp, i) => {
        if (cp) map.set(finishedRuns[i].run_id, cp)
      })
      return map
    },
    enabled: finishedRuns.length > 0,
  })
}

export function useReports(timeRange: TimeRange = '7d') {
  const { data: runsData, isLoading: runsLoading } = useReportRuns(timeRange)
  const runs: RunSummary[] =
    runsData && 'runs' in runsData ? runsData.runs : []

  const { data: checkpointMap, isLoading: cpLoading } = useReportCheckpoints(runs)

  const data = useMemo<ReportsData | null>(() => {
    if (!runsData) return null

    const reports: ReportItem[] = []
    const severityCounts: Record<FindingSeverity, number> = {
      critical: 0,
      high: 0,
      medium: 0,
      low: 0,
    }

    const runsWithFindingsSet = new Set<string>()
    const artifacts: ArtifactSummary[] = []

    for (const run of runs) {
      const cp = checkpointMap?.get(run.run_id)
      if (cp) {
        for (const finding of cp.findings) {
          reports.push({
            id: `${run.run_id}_${finding.id}`,
            run_id: run.run_id,
            task: run.task,
            severity: finding.severity,
            message: finding.message,
            source: finding.source,
            run_status: run.status,
            started_at: run.started_at,
            elapsed_ms: run.elapsed_ms,
            total_tokens: run.total_tokens,
          })
          severityCounts[finding.severity]++
          runsWithFindingsSet.add(run.run_id)
        }
      }
    }

    reports.sort((a, b) => {
      const sa = SEVERITY_ORDER.indexOf(a.severity)
      const sb = SEVERITY_ORDER.indexOf(b.severity)
      if (sa !== sb) return sa - sb
      return new Date(b.started_at).getTime() - new Date(a.started_at).getTime()
    })

    const runsCompleted = runs.filter((r) => r.status === 'completed').length
    const runsFailed = runs.filter((r) => r.status === 'failed').length

    return {
      totalFindings: reports.length,
      severityCounts,
      reports,
      artifacts,
      runsWithFindings: runsWithFindingsSet.size,
      runsCompleted,
      runsFailed,
    }
  }, [runsData, checkpointMap, runs])

  return {
    data,
    isLoading: runsLoading || (runs.length > 0 && cpLoading),
  }
}
