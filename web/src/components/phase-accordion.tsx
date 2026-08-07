import { useState } from 'react'
import { ChevronDown } from 'lucide-react'
import { cn } from '@/lib/utils'
import { AgentCard } from '@/components/agent-card'
import { useUIStore } from '@/stores/ui'
import type { PhaseDetail, AgentResultCache } from '@/api/types'

interface PhaseAccordionProps {
  phase: PhaseDetail
  defaultOpen?: boolean
  onAgentClick?: (agent: AgentResultCache) => void
}

const phaseDot: Record<string, string> = {
  completed: 'bg-primary',
  running: 'bg-blue-400 animate-pulse-dot',
  pending: 'bg-muted-foreground/40',
}

const roleBadge: Record<string, string> = {
  producer: 'bg-blue-500/12 text-blue-400',
  adversary: 'bg-amber-500/12 text-amber-400',
  voter: 'bg-purple-500/12 text-purple-400',
  default: 'bg-muted text-muted-foreground',
}

export function PhaseAccordion({ phase, defaultOpen, onAgentClick }: PhaseAccordionProps) {
  const density = useUIStore((s) => s.agentCardDensity)
  const [open, setOpen] = useState(defaultOpen ?? phase.status !== 'completed')

  const okCount = phase.agents.filter((a) => a.status === 'done').length
  const failCount = phase.agents.filter((a) => a.status === 'failed').length
  const runningCount = phase.agents.filter((a) => a.status === 'running').length

  return (
    <div
      className={cn(
        'rounded-lg border bg-card transition-colors',
        phase.status === 'running' ? 'border-blue-500/30' : 'border-border',
      )}
    >
      <button
        className="flex w-full items-center justify-between px-4 py-3 text-left"
        onClick={() => setOpen((o) => !o)}
      >
        <div className="flex items-center gap-3 min-w-0">
          <div className={cn('h-2.5 w-2.5 shrink-0 rounded-full', phaseDot[phase.status])} />
          <span className="text-sm font-medium truncate">
            Phase {phase.phase_id} — {phase.label}
          </span>
          <span
            className={cn(
              'shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium uppercase',
              roleBadge[phase.role],
            )}
          >
            {phase.role}
          </span>
        </div>
        <div className="flex items-center gap-3 shrink-0">
          {phase.agents.length > 0 && (
            <span className="text-xs text-muted-foreground font-mono">
              {okCount}/{phase.agents.length} done
              {runningCount > 0 && <span className="text-blue-400"> · {runningCount} running</span>}
              {failCount > 0 && <span className="text-destructive"> · {failCount} failed</span>}
            </span>
          )}
          <ChevronDown
            className={cn(
              'h-4 w-4 text-muted-foreground transition-transform duration-200',
              open && 'rotate-180',
            )}
          />
        </div>
      </button>
      {phase.description && open && (
        <p className="ml-6 pb-1 px-4 text-xs text-muted-foreground">{phase.description}</p>
      )}
      {open && (
        <div className={cn('px-4', density === 'compact' ? 'pb-3' : 'pb-4')}>
          {phase.agents.length === 0 ? (
            <p className="text-sm text-muted-foreground py-2">
              {phase.status === 'pending' ? 'Queued...' : 'No agents assigned'}
            </p>
          ) : (
            <div
              className={cn(
                'grid gap-3',
                density === 'compact'
                  ? 'grid-cols-[repeat(auto-fill,minmax(220px,1fr))]'
                  : 'grid-cols-[repeat(auto-fill,minmax(280px,1fr))]',
              )}
            >
              {phase.agents.map((agent) => (
                <AgentCard
                  key={agent.agent_id}
                  agent={agent}
                  compact={density === 'compact'}
                  onClick={() => onAgentClick?.(agent)}
                />
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
