import { useState } from 'react'
import { Link } from 'react-router-dom'
import { useMetrics } from '@/hooks/useMetrics'
import { DonutChart } from '@/components/charts/donut-chart'
import { BarChart } from '@/components/charts/bar-chart'
import { StatCard } from '@/components/stat-card'
import { StatusBadge } from '@/components/status-badge'
import { Card } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { formatTokens, formatElapsed, formatRelativeTime } from '@/lib/format'

type TimeRange = 'today' | '24h' | '7d' | 'all'

function MetricsSkeleton() {
  return (
    <div className="space-y-6">
      <Skeleton className="h-8 w-40" />
      <div className="grid grid-cols-4 gap-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <Skeleton key={i} className="h-24" />
        ))}
      </div>
      <div className="grid grid-cols-2 gap-6">
        <Skeleton className="h-64" />
        <Skeleton className="h-64" />
      </div>
      <Skeleton className="h-48" />
    </div>
  )
}

export function MetricsPage() {
  const [timeRange, setTimeRange] = useState<TimeRange>('7d')
  const { data: metrics, isLoading } = useMetrics(timeRange)

  if (isLoading || !metrics) {
    return <MetricsSkeleton />
  }

  const dailyRunData = metrics.dailyMetrics.map((d) => ({
    label: d.label,
    value: d.runs,
  }))

  const dailyTokenData = metrics.dailyMetrics.map((d) => ({
    label: d.label,
    value: d.tokens,
  }))

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold font-display">Metrics</h1>
        <Select value={timeRange} onValueChange={(v) => setTimeRange(v as TimeRange)}>
          <SelectTrigger className="w-32">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="today">今天</SelectItem>
            <SelectItem value="24h">24h</SelectItem>
            <SelectItem value="7d">7天</SelectItem>
            <SelectItem value="all">全部</SelectItem>
          </SelectContent>
        </Select>
      </div>

      <div className="grid grid-cols-4 gap-4">
        <StatCard
          value={metrics.totalRuns}
          label="总 Runs"
        />
        <StatCard
          value={`${metrics.successRate.toFixed(0)}%`}
          label="成功率"
        />
        <StatCard
          value={formatTokens(metrics.totalTokens)}
          label="总 Tokens"
        />
        <StatCard
          value={metrics.avgDurationMs > 0 ? formatElapsed(metrics.avgDurationMs) : '—'}
          label="平均耗时"
        />
      </div>

      <div className="grid grid-cols-2 gap-6">
        <Card className="p-5">
          <h2 className="mb-4 text-sm font-semibold text-muted-foreground uppercase tracking-wide">
            状态分布
          </h2>
          {metrics.statusBreakdown.length > 0 ? (
            <DonutChart
              segments={metrics.statusBreakdown}
              centerValue={metrics.totalRuns}
              centerLabel="Runs"
            />
          ) : (
            <p className="text-sm text-muted-foreground">暂无数据</p>
          )}
        </Card>

        <Card className="p-5">
          <h2 className="mb-4 text-sm font-semibold text-muted-foreground uppercase tracking-wide">
            每日 Token 用量
          </h2>
          <BarChart
            data={dailyTokenData}
            height={180}
            barColor="var(--color-primary)"
            formatValue={(v) => `${formatTokens(v)} tok`}
          />
        </Card>
      </div>

      <Card className="p-5">
        <h2 className="mb-4 text-sm font-semibold text-muted-foreground uppercase tracking-wide">
          每日 Run 数量
        </h2>
        <BarChart
          data={dailyRunData}
          height={140}
          barColor="var(--status-running)"
          formatValue={(v) => `${v} runs`}
        />
      </Card>

      <Card className="p-5">
        <h2 className="mb-4 text-sm font-semibold text-muted-foreground uppercase tracking-wide">
          Token 消耗 Top Runs
        </h2>
        {metrics.topRunsByTokens.length > 0 ? (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Task</TableHead>
                <TableHead className="w-24">状态</TableHead>
                <TableHead className="w-28 text-right">Tokens</TableHead>
                <TableHead className="w-32 text-right">耗时</TableHead>
                <TableHead className="w-32 text-right">开始时间</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {metrics.topRunsByTokens.map((run) => (
                <TableRow key={run.run_id}>
                  <TableCell>
                    <Link
                      to={`/runs/${run.run_id}`}
                      className="truncate text-sm font-medium text-foreground hover:text-primary"
                    >
                      {run.task}
                    </Link>
                  </TableCell>
                  <TableCell>
                    <StatusBadge status={run.status} />
                  </TableCell>
                  <TableCell className="text-right font-mono text-sm">
                    {formatTokens(run.total_tokens)}
                  </TableCell>
                  <TableCell className="text-right font-mono text-sm text-muted-foreground">
                    {formatElapsed(run.elapsed_ms)}
                  </TableCell>
                  <TableCell className="text-right font-mono text-sm text-muted-foreground">
                    {formatRelativeTime(run.started_at)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        ) : (
          <p className="text-sm text-muted-foreground">暂无数据</p>
        )}
      </Card>
    </div>
  )
}
