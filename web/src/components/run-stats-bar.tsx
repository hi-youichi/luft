import { cn } from '@/lib/utils'
import { formatElapsed } from '@/lib/format'
import { Check, X, Loader, Wrench, Clock, Cpu } from 'lucide-react'
import type { RunCheckpoint } from '@/api/types'

interface RunStatsBarProps {
  run: RunCheckpoint
  className?: string
}

interface StatItemProps {
  icon: typeof Check
  iconClass: string
  label: string
  value: string | number
}

function StatItem({ icon: Icon, iconClass, label, value }: StatItemProps) {
  return (
    <div className="flex items-center gap-2">
      <Icon className={cn('h-3.5 w-3.5', iconClass)} />
      <div className="flex flex-col">
        <span className="font-mono text-sm font-medium text-foreground leading-tight">{value}</span>
        <span className="text-[10px] uppercase tracking-wide text-muted-foreground leading-tight">{label}</span>
      </div>
    </div>
  )
}

export function RunStatsBar({ run, className }: RunStatsBarProps) {
  const allAgents = run.phases.flatMap((p) => p.agents)
  const doneCount = allAgents.filter((a) => a.status === 'done').length
  const failedCount = allAgents.filter((a) => a.status === 'failed').length
  const runningCount = allAgents.filter((a) => a.status === 'running').length
  const totalAgents = allAgents.length
  const totalToolCalls = allAgents.reduce((sum, a) => sum + a.tool_calls, 0)

  return (
    <div className={cn('flex flex-wrap items-center gap-x-6 gap-y-2', className)}>
      <StatItem icon={Cpu} iconClass="text-muted-foreground" label="Agents" value={totalAgents} />
      <StatItem icon={Check} iconClass="text-primary" label="Done" value={doneCount} />
      {runningCount > 0 && <StatItem icon={Loader} iconClass="text-blue-400 animate-spin" label="Running" value={runningCount} />}
      {failedCount > 0 && <StatItem icon={X} iconClass="text-destructive" label="Failed" value={failedCount} />}
      <StatItem icon={Wrench} iconClass="text-muted-foreground" label="Tool Calls" value={totalToolCalls} />
      <StatItem icon={Clock} iconClass="text-muted-foreground" label="Elapsed" value={formatElapsed(run.elapsed_ms)} />
    </div>
  )
}
