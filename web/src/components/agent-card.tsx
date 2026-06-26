import { cn } from '@/lib/utils'
import { formatTokens, formatElapsed } from '@/lib/format'
import type { AgentResultCache } from '@/api/types'
import { Check, X, Loader, Circle } from 'lucide-react'

const roleStyles: Record<string, string> = {
  producer: 'bg-blue-500/12 text-blue-400',
  adversary: 'bg-amber-500/12 text-amber-400',
  voter: 'bg-purple-500/12 text-purple-400',
  default: 'bg-muted text-muted-foreground',
}

const statusIcon: Record<string, typeof Check> = {
  done: Check,
  failed: X,
  running: Loader,
  pending: Circle,
}

interface AgentCardProps {
  agent: AgentResultCache
  onClick?: () => void
}

export function AgentCard({ agent, onClick }: AgentCardProps) {
  const roleClass = roleStyles[agent.role] ?? roleStyles.default
  const StatusIcon = statusIcon[agent.status] ?? Circle

  return (
    <div
      onClick={onClick}
      className={cn(
        'group cursor-pointer rounded-lg border bg-card p-3 transition-all hover:border-primary/40 hover:shadow-md',
        agent.status === 'running' && 'border-blue-500/40 border-l-2 border-l-blue-500',
        agent.status === 'failed' && 'border-destructive/40',
        agent.status === 'pending' && 'opacity-50',
      )}
    >
      <div className="flex items-center justify-between mb-2">
        <span className={cn('rounded px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide', roleClass)}>
          {agent.role}
        </span>
        <StatusIcon
          className={cn(
            'h-3.5 w-3.5',
            agent.status === 'done' && 'text-primary',
            agent.status === 'failed' && 'text-destructive',
            agent.status === 'running' && 'text-blue-400 animate-spin',
            agent.status === 'pending' && 'text-muted-foreground',
          )}
        />
      </div>
      <div className="text-sm font-medium truncate">{agent.agent_id}</div>
      {agent.description && (
        <div className="mt-0.5 text-xs text-muted-foreground truncate">{agent.description}</div>
      )}
      {agent.output_preview && (
        <div className="mt-1.5">
          <span className="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">Output</span>
          <div className="mt-0.5 text-xs text-muted-foreground line-clamp-1">{agent.output_preview}</div>
        </div>
      )}
      {agent.error && (
        <div className="mt-1.5 text-xs text-destructive line-clamp-1">{agent.error}</div>
      )}
      <div className="mt-2 flex items-center justify-between font-mono text-xs text-muted-foreground">
        <span>{formatTokens(agent.tokens.input + agent.tokens.output)} tok</span>
        <span>{agent.tool_calls} calls</span>
        {agent.elapsed_ms > 0 && <span>{formatElapsed(agent.elapsed_ms)}</span>}
      </div>
    </div>
  )
}
