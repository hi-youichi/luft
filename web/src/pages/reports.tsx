import { useState } from 'react'
import { Link } from 'react-router-dom'
import { AlertOctagon, CheckCircle2, XCircle, Bug } from 'lucide-react'
import { useReports } from '@/hooks/useReports'
import type { FindingSeverity } from '@/hooks/useReports'
import { StatCard } from '@/components/stat-card'
import { StatusBadge } from '@/components/status-badge'
import { Card } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { formatRelativeTime } from '@/lib/format'
import { cn } from '@/lib/utils'

type TimeRange = 'today' | '24h' | '7d' | 'all'
type SeverityFilter = FindingSeverity | 'all'

const SEVERITY_CONFIG: Record<FindingSeverity, { label: string; color: string; bgColor: string; dotColor: string }> = {
  critical: { label: '严重', color: 'text-destructive', bgColor: 'bg-destructive', dotColor: 'bg-destructive' },
  high: { label: '高', color: 'text-amber-400', bgColor: 'bg-amber-400', dotColor: 'bg-amber-400' },
  medium: { label: '中', color: 'text-blue-400', bgColor: 'bg-blue-400', dotColor: 'bg-blue-400' },
  low: { label: '低', color: 'text-muted-foreground', bgColor: 'bg-muted-foreground', dotColor: 'bg-muted-foreground' },
}

function SeverityBadge({ severity }: { severity: FindingSeverity }) {
  const cfg = SEVERITY_CONFIG[severity]
  return (
    <span className={cn('inline-flex items-center gap-1.5 rounded px-2 py-0.5 text-xs font-medium', cfg.color)}>
      <span className={cn('h-1.5 w-1.5 rounded-full', cfg.dotColor)} />
      {cfg.label}
    </span>
  )
}

function ReportsSkeleton() {
  return (
    <div className="space-y-6">
      <Skeleton className="h-8 w-40" />
      <div className="grid grid-cols-4 gap-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <Skeleton key={i} className="h-24" />
        ))}
      </div>
      <Skeleton className="h-32" />
      <Skeleton className="h-64" />
    </div>
  )
}

export function ReportsPage() {
  const [timeRange, setTimeRange] = useState<TimeRange>('7d')
  const [severity, setSeverity] = useState<SeverityFilter>('all')
  const { data, isLoading } = useReports(timeRange)

  if (isLoading || !data) {
    return <ReportsSkeleton />
  }

  const filteredReports = severity === 'all'
    ? data.reports
    : data.reports.filter((r) => r.severity === severity)

  const maxSeverityCount = Math.max(1, ...Object.values(data.severityCounts))

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold font-display">Reports</h1>
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
          value={data.totalFindings}
          label="总 Findings"
        />
        <StatCard
          value={data.runsWithFindings}
          label="有 Findings 的 Runs"
        />
        <StatCard
          value={data.runsCompleted}
          label="已完成 Runs"
        />
        <StatCard
          value={data.runsFailed}
          label="失败 Runs"
        />
      </div>

      {data.totalFindings > 0 && (
        <Card className="p-5">
          <h2 className="mb-4 text-sm font-semibold text-muted-foreground uppercase tracking-wide">
            严重程度分布
          </h2>
          <div className="space-y-3">
            {(Object.keys(data.severityCounts) as FindingSeverity[])
              .filter((s) => data.severityCounts[s] > 0)
              .map((sev) => {
                const cfg = SEVERITY_CONFIG[sev]
                const count = data.severityCounts[sev]
                const pct = (count / maxSeverityCount) * 100
                return (
                  <div key={sev} className="flex items-center gap-3">
                    <span className={cn('w-12 text-xs font-medium', cfg.color)}>{cfg.label}</span>
                    <div className="flex-1 h-6 rounded bg-bg-base overflow-hidden">
                      <div
                        className={cn('h-full rounded transition-all', cfg.bgColor, 'opacity-60')}
                        style={{ width: `${pct}%` }}
                      />
                    </div>
                    <span className="w-8 text-right font-mono text-sm text-muted-foreground">{count}</span>
                  </div>
                )
              })}
          </div>
        </Card>
      )}

      <Card className="p-5">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-sm font-semibold text-muted-foreground uppercase tracking-wide">
            Findings 详情
          </h2>
          <Select value={severity} onValueChange={(v) => setSeverity(v as SeverityFilter)}>
            <SelectTrigger className="w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">全部</SelectItem>
              <SelectItem value="critical">严重</SelectItem>
              <SelectItem value="high">高</SelectItem>
              <SelectItem value="medium">中</SelectItem>
              <SelectItem value="low">低</SelectItem>
            </SelectContent>
          </Select>
        </div>

        {filteredReports.length > 0 ? (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-20">级别</TableHead>
                <TableHead>描述</TableHead>
                <TableHead className="w-40">Task</TableHead>
                <TableHead className="w-24">状态</TableHead>
                <TableHead className="w-28">Agent</TableHead>
                <TableHead className="w-24 text-right">时间</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {filteredReports.map((r) => (
                <TableRow key={r.id}>
                  <TableCell>
                    <SeverityBadge severity={r.severity} />
                  </TableCell>
                  <TableCell className="text-sm">{r.message}</TableCell>
                  <TableCell>
                    <Link
                      to={`/runs/${r.run_id}`}
                      className="truncate text-sm font-medium text-foreground hover:text-primary block max-w-[160px]"
                    >
                      {r.task}
                    </Link>
                  </TableCell>
                  <TableCell>
                    <StatusBadge status={r.run_status} />
                  </TableCell>
                  <TableCell className="font-mono text-xs text-muted-foreground">
                    {r.source ?? '—'}
                  </TableCell>
                  <TableCell className="text-right font-mono text-xs text-muted-foreground">
                    {formatRelativeTime(r.started_at)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        ) : (
          <div className="py-12 text-center text-muted-foreground">
            <Bug className="mx-auto mb-2 h-8 w-8 opacity-40" />
            <p className="text-sm">
              {data.totalFindings === 0 ? '暂无 Findings' : '当前筛选条件下无结果'}
            </p>
          </div>
        )}
      </Card>

      {data.reports.length > 0 && (
        <Card className="p-5">
          <h2 className="mb-4 text-sm font-semibold text-muted-foreground uppercase tracking-wide">
            Run 概况
          </h2>
          <div className="grid grid-cols-3 gap-4">
            <div className="flex items-center gap-3 rounded-lg border border-border bg-bg-base p-3">
              <div className="flex h-8 w-8 items-center justify-center rounded bg-primary/10">
                <CheckCircle2 className="h-4 w-4 text-primary" />
              </div>
              <div>
                <div className="text-lg font-bold font-display">{data.runsCompleted}</div>
                <div className="text-xs text-muted-foreground">成功</div>
              </div>
            </div>
            <div className="flex items-center gap-3 rounded-lg border border-border bg-bg-base p-3">
              <div className="flex h-8 w-8 items-center justify-center rounded bg-destructive/10">
                <XCircle className="h-4 w-4 text-destructive" />
              </div>
              <div>
                <div className="text-lg font-bold font-display">{data.runsFailed}</div>
                <div className="text-xs text-muted-foreground">失败</div>
              </div>
            </div>
            <div className="flex items-center gap-3 rounded-lg border border-border bg-bg-base p-3">
              <div className="flex h-8 w-8 items-center justify-center rounded bg-amber-500/10">
                <AlertOctagon className="h-4 w-4 text-amber-400" />
              </div>
              <div>
                <div className="text-lg font-bold font-display">{data.totalFindings}</div>
                <div className="text-xs text-muted-foreground">Findings 总计</div>
              </div>
            </div>
          </div>
        </Card>
      )}
    </div>
  )
}
