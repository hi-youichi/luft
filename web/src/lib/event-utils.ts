import type { AgentEvent } from '@/api/types'

export interface EventDisplay {
  ts: string
  indent: number
  icon: string
  iconColor: string
  text: string
  detail?: string
}

export function eventToDisplay(event: AgentEvent): EventDisplay {
  switch (event.type) {
    case 'RunStarted':
      return { ts: event.ts, indent: 0, icon: '▶', iconColor: 'text-primary', text: 'Run started', detail: event.task }
    case 'PhaseStarted':
      return { ts: event.ts, indent: 1, icon: '▶', iconColor: 'text-blue-400', text: `Phase ${event.phase_id} started`, detail: `${event.label} (${event.planned} agents)` }
    case 'AgentStarted':
      return { ts: event.ts, indent: 2, icon: '→', iconColor: 'text-muted-foreground', text: `${event.agent_id} started`, detail: event.role }
    case 'AgentProgress':
      return { ts: event.ts, indent: 2, icon: '·', iconColor: 'text-muted-foreground', text: `${event.agent_id} progress` }
    case 'AgentDone':
      return {
        ts: event.ts, indent: 2,
        icon: event.status === 'done' ? '←' : '✗',
        iconColor: event.status === 'done' ? 'text-primary' : 'text-destructive',
        text: `${event.agent_id} ${event.status === 'done' ? 'done' : 'failed'}`,
      }
    case 'PhaseDone':
      return { ts: event.ts, indent: 1, icon: '✓', iconColor: event.failed > 0 ? 'text-amber-400' : 'text-primary', text: `Phase ${event.phase_id} done`, detail: `${event.ok} ok, ${event.failed} failed` }
    case 'RunDone':
      return { ts: event.ts, indent: 0, icon: event.status === 'completed' ? '✓' : '✗', iconColor: event.status === 'completed' ? 'text-primary' : 'text-destructive', text: `Run ${event.status}` }
    default:
      return { ts: '', indent: 0, icon: '·', iconColor: 'text-muted-foreground', text: 'unknown' }
  }
}
