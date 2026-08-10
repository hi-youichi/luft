import { cn } from '@/lib/utils'
import { Check, Loader, Circle } from 'lucide-react'
import type { PhaseDetail } from '@/api/types'

const statusConfig = {
  completed: { icon: Check, dotClass: 'bg-primary', iconClass: 'text-primary', lineClass: 'bg-primary/30' },
  running: { icon: Loader, dotClass: 'bg-blue-400', iconClass: 'text-blue-400 animate-spin', lineClass: 'bg-blue-400/30' },
  pending: { icon: Circle, dotClass: 'bg-muted-foreground/30', iconClass: 'text-muted-foreground/40', lineClass: 'bg-border' },
} as const

interface TimelineProps {
  phases: PhaseDetail[]
  selectedPhaseId?: number | null
  onSelectPhase?: (phaseId: number) => void
}

export function Timeline({ phases, selectedPhaseId, onSelectPhase }: TimelineProps) {
  return (
    <div className="space-y-0">
      {phases.map((phase, idx) => {
        const cfg = statusConfig[phase.status]
        const Icon = cfg.icon
        const isSelected = selectedPhaseId === phase.phase_id
        const isLast = idx === phases.length - 1
        const agentCount = phase.agents.length
        const doneCount = phase.agents.filter((a) => a.status === 'done').length

        return (
          <div
            key={phase.phase_id}
            className={cn('relative flex gap-3', onSelectPhase && 'cursor-pointer')}
            onClick={() => onSelectPhase?.(phase.phase_id)}
          >
            <div className="flex flex-col items-center">
              <div
                className={cn(
                  'flex h-6 w-6 shrink-0 items-center justify-center rounded-full border-2 transition-all',
                  phase.status === 'running'
                    ? 'border-blue-500/40 bg-blue-500/10'
                    : phase.status === 'completed'
                      ? 'border-primary/30 bg-primary/10'
                      : 'border-border bg-card',
                  isSelected && 'ring-2 ring-primary/40 ring-offset-2 ring-offset-card',
                )}
              >
                <Icon className={cn('h-3 w-3', cfg.iconClass)} />
              </div>
              {!isLast && (
                <div className={cn('w-0.5 flex-1 my-1', cfg.lineClass)} />
              )}
            </div>
            <div className={cn('pb-4 min-w-0 flex-1', isLast && 'pb-0')}>
              <div
                className={cn(
                  'text-sm font-medium leading-6',
                  phase.status === 'pending' ? 'text-muted-foreground' : 'text-foreground',
                )}
              >
                {phase.label}
              </div>
              {agentCount > 0 && (
                <div className="text-xs text-muted-foreground font-mono">
                  {doneCount}/{agentCount} agents
                  {phase.status === 'running' && (
                    <span className="text-blue-400 ml-1">· active</span>
                  )}
                </div>
              )}
              {phase.status === 'pending' && (
                <div className="text-xs text-muted-foreground/50">Queued</div>
              )}
            </div>
          </div>
        )
      })}
    </div>
  )
}
