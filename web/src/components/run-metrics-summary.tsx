import { useMemo } from 'react'
import { CheckCircle2, XCircle, Clock, Zap, Wrench, TrendingUp } from 'lucide-react'
import { cn } from '@/lib/utils'
import { formatTokens, formatElapsed } from '@/lib/format'
import type { RunCheckpoint } from '@/api/types'

interface RunMetricsSummaryProps {
  run: RunCheckpoint
  className?: string
}

interface MetricItemProps {
  icon: typeof CheckCircle2
  label: string
  value: string
  subValue?: string
  iconClass: string
}

function MetricItem({ icon: Icon, label, value, subValue, iconClass }: MetricItemProps) {
  return (
    <div className="flex items-center gap-2.5">
      <div className={cn('flex h-8 w-8 shrink-0 items-center justify-center rounded-lg', iconClass)}>
        <Icon className="h-4 w-4" />
      </div>
      <div className="min-w-0">
        <div className="text-xs text-muted-foreground truncate">{label}</div>
        <div className="flex items-baseline gap-1">
          <span className="text-sm font-mono font-semibold text-foreground">{value}</span>
          {subValue && <span className="text-[10px] text-muted-foreground">{subValue}</span>}
        </div>
      </div>
    </div>
  )
}

export function RunMetricsSummary({ run, className }: RunMetricsSummaryProps) {
  const metrics = useMemo(() => {
    const allAgents = run.phases.flatMap((p) => p.agents)
    const total = allAgents.length
    const done = allAgents.filter((a) => a.status === 'done').length
    const failed = allAgents.filter((a) => a.status === 'failed').length
    const running = allAgents.filter((a) => a.status === 'running').length

    const successRate = total > 0 ? Math.round((done / total) * 100) : 0

    const completedAgents = allAgents.filter((a) => a.elapsed_ms > 0)
    const avgMs = completedAgents.length > 0
      ? Math.round(completedAgents.reduce((s, a) => s + a.elapsed_ms, 0) / completedAgents.length)
      : 0

    const fastest = completedAgents.length > 0
      ? Math.min(...completedAgents.map((a) => a.elapsed_ms))
      : 0
    const slowest = completedAgents.length > 0
      ? Math.max(...completedAgents.map((a) => a.elapsed_ms))
      : 0

    const totalToolCalls = allAgents.reduce((s, a) => s + a.tool_calls, 0)
    const avgTokens = total > 0
      ? Math.round(allAgents.reduce((s, a) => s + a.tokens.input + a.tokens.output, 0) / total)
      : 0

    return { total, done, failed, running, successRate, avgMs, fastest, slowest, totalToolCalls, avgTokens }
  }, [run])

  return (
    <div className={cn('grid gap-3', className)}>
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-3">
        <MetricItem
          icon={CheckCircle2}
          label="Success Rate"
          value={`${metrics.successRate}%`}
          subValue={`${metrics.done}/${metrics.total}`}
          iconClass="bg-primary/10 text-primary"
        />
        <MetricItem
          icon={TrendingUp}
          label="Avg Tokens"
          value={formatTokens(metrics.avgTokens)}
          subValue="/ agent"
          iconClass="bg-blue-500/10 text-blue-400"
        />
        <MetricItem
          icon={Clock}
          label="Avg Duration"
          value={formatElapsed(metrics.avgMs)}
          subValue={`${formatElapsed(metrics.fastest)}–${formatElapsed(metrics.slowest)}`}
          iconClass="bg-amber-500/10 text-amber-400"
        />
        <MetricItem
          icon={Wrench}
          label="Tool Calls"
          value={String(metrics.totalToolCalls)}
          subValue="total"
          iconClass="bg-cyan-500/10 text-cyan-400"
        />
        <MetricItem
          icon={XCircle}
          label="Failures"
          value={String(metrics.failed)}
          iconClass={cn(metrics.failed > 0 ? 'bg-destructive/10 text-destructive' : 'bg-muted text-muted-foreground')}
        />
        <MetricItem
          icon={Zap}
          label="Active"
          value={String(metrics.running)}
          iconClass={cn(metrics.running > 0 ? 'bg-blue-500/10 text-blue-400' : 'bg-muted text-muted-foreground')}
        />
      </div>
    </div>
  )
}
