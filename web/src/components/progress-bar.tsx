import { cn } from '@/lib/utils'

interface ProgressBarProps {
  current: number
  total: number
  showShimmer?: boolean
  className?: string
}

export function ProgressBar({ current, total, showShimmer = false, className }: ProgressBarProps) {
  const pct = total > 0 ? Math.round((current / total) * 100) : 0
  return (
    <div className={cn('flex items-center gap-2', className)}>
      <span className="text-xs font-mono text-muted-foreground whitespace-nowrap">
        {current}/{total}
      </span>
      <div className="relative h-1.5 w-24 overflow-hidden rounded-full bg-muted">
        <div
          className={cn(
            'h-full rounded-full transition-all duration-300',
            showShimmer ? 'shimmer bg-primary' : 'bg-primary'
          )}
          style={{ width: `${pct}%` }}
        />
      </div>
      <span className="text-xs font-mono text-muted-foreground w-8 text-right">{pct}%</span>
    </div>
  )
}
