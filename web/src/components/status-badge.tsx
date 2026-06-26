import { Loader, Check, X, Circle, Ban } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { RunStatus, AgentStatus } from '@/api/types'

type Status = RunStatus | AgentStatus

const config: Record<Status, { icon: typeof Loader; label: string; className: string }> = {
  running:   { icon: Loader, label: '运行中',  className: 'bg-blue-500/12 text-blue-400' },
  completed: { icon: Check,  label: '完成',    className: 'bg-primary/12 text-primary' },
  done:      { icon: Check,  label: '完成',    className: 'bg-primary/12 text-primary' },
  failed:    { icon: X,      label: '失败',    className: 'bg-destructive/12 text-destructive' },
  pending:   { icon: Circle, label: '等待中',  className: 'bg-muted text-muted-foreground' },
  cancelled: { icon: Ban,    label: '已取消',  className: 'bg-amber-500/12 text-amber-400' },
}

export function StatusBadge({ status, className }: { status: Status; className?: string }) {
  const cfg = config[status] ?? config.pending
  const Icon = cfg.icon
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded px-2 py-0.5 text-xs font-medium',
        cfg.className,
        className
      )}
    >
      <Icon className={cn('h-3 w-3', status === 'running' && 'animate-spin')} />
      {cfg.label}
    </span>
  )
}
