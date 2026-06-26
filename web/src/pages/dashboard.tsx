import { Link } from 'react-router-dom'
import { useDashboardStats } from '@/hooks/useDashboardStats'
import { StatCard } from '@/components/stat-card'
import { StatusBadge } from '@/components/status-badge'
import { ProgressBar } from '@/components/progress-bar'
import { Card } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { formatTokens, formatElapsed, formatRelativeTime } from '@/lib/format'
import type { RunSummary } from '@/api/types'

function RunMiniCard({ run }: { run: RunSummary }) {
  return (
    <Link to={`/runs/${run.run_id}`}>
      <Card className="p-4 hover:border-primary/40 transition-colors cursor-pointer">
        <div className="flex items-start justify-between mb-2">
          <span className="text-sm font-medium truncate flex-1 mr-2">{run.task}</span>
          <StatusBadge status={run.status} />
        </div>
        <ProgressBar
          current={run.current_phase}
          total={run.total_phases}
          showShimmer={run.status === 'running'}
        />
        <div className="mt-2 flex items-center gap-4 text-xs font-mono text-muted-foreground">
          <span>{formatTokens(run.total_tokens)} tok</span>
          <span>{run.status === 'running' ? formatElapsed(run.elapsed_ms) : formatRelativeTime(run.started_at)}</span>
        </div>
      </Card>
    </Link>
  )
}

export function Dashboard() {
  const { data: stats, isLoading } = useDashboardStats()

  if (isLoading || !stats) {
    return (
      <div className="grid grid-cols-4 gap-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <Skeleton key={i} className="h-24" />
        ))}
      </div>
    )
  }

  return (
    <div className="space-y-6">
      <h1 className="text-xl font-semibold font-display">Dashboard</h1>

      <div className="grid grid-cols-2 gap-6">
        <div>
          <h2 className="mb-3 text-sm font-semibold text-muted-foreground uppercase tracking-wide">
            活跃 Runs ({stats.active_runs.length})
          </h2>
          <div className="flex flex-col gap-3">
            {stats.active_runs.length === 0 ? (
              <p className="text-sm text-muted-foreground">暂无活跃 Run</p>
            ) : (
              stats.active_runs.map((run) => <RunMiniCard key={run.run_id} run={run} />)
            )}
          </div>
        </div>

        <div>
          <h2 className="mb-3 text-sm font-semibold text-muted-foreground uppercase tracking-wide">
            最近完成
          </h2>
          <div className="flex flex-col gap-3">
            {stats.recent_runs.length === 0 ? (
              <p className="text-sm text-muted-foreground">暂无历史 Run</p>
            ) : (
              stats.recent_runs.map((run) => <RunMiniCard key={run.run_id} run={run} />)
            )}
          </div>
        </div>
      </div>

      <div>
        <h2 className="mb-3 text-sm font-semibold text-muted-foreground uppercase tracking-wide">
          今日统计
        </h2>
        <div className="grid grid-cols-4 gap-4">
          <StatCard value={stats.today_runs} label="Runs" />
          <StatCard value={formatTokens(stats.today_tokens)} label="Tokens" />
          <StatCard value={stats.today_success} label="成功" />
          <StatCard value={stats.today_failed} label="失败" />
        </div>
      </div>
    </div>
  )
}
