import { useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { RadioTower } from 'lucide-react'
import { useLiveFeed } from '@/hooks/useLiveFeed'
import { StatCard } from '@/components/stat-card'
import { StatusBadge } from '@/components/status-badge'
import { ProgressBar } from '@/components/progress-bar'
import { Card } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { eventToDisplay } from '@/lib/event-utils'
import { formatTokens, formatElapsed, formatTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { AgentEvent, RunSummary } from '@/api/types'

type EventFilter = 'all' | 'run' | 'phase' | 'agent'

const filterMap: Record<EventFilter, (e: AgentEvent) => boolean> = {
  all: () => true,
  run: (e) => e.type === 'RunStarted' || e.type === 'RunDone',
  phase: (e) => e.type === 'PhaseStarted' || e.type === 'PhaseDone',
  agent: (e) => e.type === 'AgentStarted' || e.type === 'AgentProgress' || e.type === 'AgentDone' || e.type === 'AcpRequest',
}

function RunMonitorCard({ run }: { run: RunSummary }) {
  return (
    <Link to={`/runs/${run.run_id}`}>
      <Card className="p-3 hover:border-primary/40 transition-colors">
        <div className="flex items-start justify-between mb-2">
          <span className="text-sm font-medium truncate flex-1 mr-2">{run.task}</span>
          <StatusBadge status={run.status} />
        </div>
        <ProgressBar
          current={run.current_phase}
          total={run.total_phases}
          showShimmer
        />
        <div className="mt-2 flex items-center gap-3 text-xs font-mono text-muted-foreground">
          <span className="text-primary animate-pulse-dot">●</span>
          <span>{formatTokens(run.total_tokens)} tok</span>
          <span>{formatElapsed(run.elapsed_ms)}</span>
          <span className="ml-auto text-muted-foreground/50">#{run.run_id}</span>
        </div>
      </Card>
    </Link>
  )
}

function LiveEventFeed({ events }: { events: AgentEvent[] }) {
  const [filter, setFilter] = useState<EventFilter>('all')
  const scrollRef = useRef<HTMLDivElement>(null)

  const filtered = events.filter(filterMap[filter])
  const displays = filtered.map((e) => ({ ...eventToDisplay(e), run_id: e.run_id }))

  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight
    }
  }, [filtered.length])

  const filters: { key: EventFilter; label: string }[] = [
    { key: 'all', label: 'All' },
    { key: 'run', label: 'Run' },
    { key: 'phase', label: 'Phase' },
    { key: 'agent', label: 'Agent' },
  ]

  return (
    <div className="rounded-lg border border-border bg-card overflow-hidden flex flex-col">
      <div className="flex items-center justify-between px-4 py-2.5 border-b border-border">
        <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
          Live Events · {filtered.length}
        </span>
        <div className="flex items-center gap-1">
          {filters.map(({ key, label }) => (
            <button
              key={key}
              onClick={() => setFilter(key)}
              className={cn(
                'rounded px-2 py-0.5 text-[11px] font-medium transition-colors',
                filter === key
                  ? 'bg-primary/15 text-primary'
                  : 'text-muted-foreground hover:text-foreground hover:bg-hover/50'
              )}
            >
              {label}
            </button>
          ))}
        </div>
      </div>
      <div ref={scrollRef} className="flex-1 max-h-[520px] overflow-y-auto p-2 font-mono text-xs">
        {displays.length === 0 ? (
          <div className="text-muted-foreground text-center py-8">No events yet</div>
        ) : (
          displays.map((d, i) => (
            <div
              key={i}
              className={cn(
                'flex items-start gap-2 py-0.5 px-2 rounded hover:bg-hover/30',
                d.indent === 1 && 'ml-4',
                d.indent === 2 && 'ml-8',
              )}
            >
              <span className="text-muted-foreground whitespace-nowrap">{formatTime(d.ts)}</span>
              <span className={cn('select-none', d.iconColor)}>{d.icon}</span>
              <span className="text-foreground">{d.text}</span>
              {d.detail && <span className="text-muted-foreground">— {d.detail}</span>}
              <span className="ml-auto text-[10px] text-muted-foreground/50 whitespace-nowrap">
                #{d.run_id}
              </span>
            </div>
          ))
        )}
      </div>
    </div>
  )
}

export function LivePage() {
  const { runs, events, wsTotal, wsConnected, isLoading } = useLiveFeed()

  const totalTokens = runs.reduce((sum, r) => sum + r.total_tokens, 0)
  const maxElapsed = runs.length > 0 ? Math.max(...runs.map((r) => r.elapsed_ms)) : 0

  if (isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-7 w-48" />
        <div className="grid grid-cols-4 gap-4">
          {Array.from({ length: 4 }).map((_, i) => (
            <Skeleton key={i} className="h-24" />
          ))}
        </div>
        <div className="grid grid-cols-[1fr_1.3fr] gap-4">
          <Skeleton className="h-64" />
          <Skeleton className="h-64" />
        </div>
      </div>
    )
  }

  if (runs.length === 0) {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <RadioTower className="h-5 w-5 text-muted-foreground" />
          <h1 className="text-xl font-semibold font-display">Live Monitor</h1>
        </div>
        <Card className="p-16 text-center">
          <RadioTower className="mx-auto h-10 w-10 text-muted-foreground/40" />
          <p className="mt-4 text-sm text-muted-foreground">No active runs</p>
          <p className="mt-1 text-xs text-muted-foreground/60">Live monitoring will activate when runs are in progress</p>
        </Card>
      </div>
    )
  }

  return (
    <div className="space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-2">
            <span className="relative flex h-2.5 w-2.5">
              <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-primary opacity-60" />
              <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-primary" />
            </span>
            <h1 className="text-xl font-semibold font-display">Live Monitor</h1>
          </div>
          <span className="text-xs font-mono text-muted-foreground">
            {wsConnected}/{wsTotal} streams · {events.length} events
          </span>
        </div>
      </div>

      {/* Stats */}
      <div className="grid grid-cols-4 gap-4">
        <StatCard value={runs.length} label="Active Runs" />
        <StatCard value={`${wsConnected}/${wsTotal}`} label="WS Streams" />
        <StatCard value={formatTokens(totalTokens)} label="Tokens In-Flight" />
        <StatCard value={formatElapsed(maxElapsed)} label="Longest Run" />
      </div>

      {/* Main content */}
      <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.3fr)] gap-4">
        {/* Active Runs */}
        <div className="space-y-3">
          <div className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
            Active Runs ({runs.length})
          </div>
          <div className="space-y-2">
            {runs.map((run) => (
              <RunMonitorCard key={run.run_id} run={run} />
            ))}
          </div>
        </div>

        {/* Event Stream */}
        <LiveEventFeed events={events} />
      </div>
    </div>
  )
}
