import { useState } from 'react'
import { useParams, Link } from 'react-router-dom'
import { ArrowLeft, ChevronRight } from 'lucide-react'
import { useRunDetail, useRunEvents } from '@/hooks/useRunDetail'
import { StatusBadge } from '@/components/status-badge'
import { ProgressBar } from '@/components/progress-bar'
import { AgentCard } from '@/components/agent-card'
import { EventStream } from '@/components/event-stream'
import { DetailDrawer } from '@/components/detail-drawer'
import { Skeleton } from '@/components/ui/skeleton'
import { Button } from '@/components/ui/button'
import { formatTokens, formatElapsed, formatRelativeTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { AgentResultCache } from '@/api/types'

const phaseDot: Record<string, string> = {
  completed: 'bg-primary',
  running: 'bg-blue-400 animate-pulse-dot',
  pending: 'bg-muted-foreground/40',
}

export function RunDetailPage() {
  const { runId } = useParams<{ runId: string }>()
  const { data: run, isLoading } = useRunDetail(runId!)
  const { data: events } = useRunEvents(runId!)
  const [selectedAgent, setSelectedAgent] = useState<AgentResultCache | null>(null)
  const [drawerOpen, setDrawerOpen] = useState(false)

  if (isLoading || !run) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-8 w-64" />
        <Skeleton className="h-16" />
        <div className="flex gap-4">
          <Skeleton className="h-64 w-48" />
          <Skeleton className="h-64 flex-1" />
        </div>
      </div>
    )
  }

  function openAgent(agent: AgentResultCache) {
    setSelectedAgent(agent)
    setDrawerOpen(true)
  }

  const findingsCount = run.findings.length

  return (
    <div className="space-y-4">
      {/* Breadcrumb + back */}
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <Button variant="ghost" size="sm" asChild>
          <Link to="/runs"><ArrowLeft className="h-3.5 w-3.5" /> 返回列表</Link>
        </Button>
        <ChevronRight className="h-3 w-3" />
        <span className="text-foreground font-mono">#{runId}</span>
      </div>

      {/* Run Header */}
      <div className="rounded-lg border border-border bg-card p-5">
        <div className="flex items-start justify-between mb-3">
          <div>
            <h1 className="text-xl font-semibold font-display">{run.task}</h1>
            <div className="mt-1 flex items-center gap-3 text-xs text-muted-foreground">
              <span>{formatRelativeTime(run.started_at)}</span>
              <span>·</span>
              <span>{formatElapsed(run.elapsed_ms)}</span>
            </div>
          </div>
          <div className="flex items-center gap-4">
            <div className="text-right">
              <div className="font-mono text-lg font-semibold text-foreground">
                {formatTokens(run.total_tokens)}
              </div>
              <div className="text-xs text-muted-foreground">tokens</div>
            </div>
            <StatusBadge status={run.status} />
          </div>
        </div>
        <ProgressBar
          current={run.current_phase}
          total={run.phases.length}
          showShimmer={run.status === 'running'}
          className="text-sm"
        />
      </div>

      {/* Main layout: sidebar + content */}
      <div className="flex gap-4">
        {/* Sidebar */}
        <aside className="w-52 shrink-0 space-y-4">
          <div className="rounded-lg border border-border bg-card p-4">
            <div className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-3">
              Timeline
            </div>
            <div className="space-y-2">
              {run.phases.map((phase) => (
                <div key={phase.phase_id} className="flex items-center gap-2.5">
                  <div className={cn('h-2 w-2 rounded-full', phaseDot[phase.status])} />
                  <span className={cn(
                    'text-sm',
                    phase.status === 'running' ? 'text-foreground font-medium' : 'text-muted-foreground'
                  )}>
                    {phase.label}
                  </span>
                </div>
              ))}
            </div>
          </div>

          {findingsCount > 0 && (
            <div className="rounded-lg border border-border bg-card p-4">
              <div className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                Findings
              </div>
              <div className="text-2xl font-bold font-display text-foreground">{findingsCount}</div>
              <div className="mt-2 space-y-1">
                {['critical', 'high', 'medium', 'low'].map((sev) => {
                  const count = run.findings.filter(f => f.severity === sev).length
                  if (count === 0) return null
                  return (
                    <div key={sev} className="flex items-center justify-between text-xs">
                      <span className={cn(
                        sev === 'critical' && 'text-destructive',
                        sev === 'high' && 'text-amber-400',
                        sev === 'medium' && 'text-blue-400',
                        sev === 'low' && 'text-muted-foreground',
                      )}>
                        {sev}
                      </span>
                      <span className="font-mono text-muted-foreground">{count}</span>
                    </div>
                  )
                })}
              </div>
            </div>
          )}
        </aside>

        {/* Content area */}
        <div className="flex-1 min-w-0 space-y-2">
          {/* Phase accordions (always expanded for simplicity) */}
          {run.phases.map((phase) => {
            const okCount = phase.agents.filter(a => a.status === 'done').length
            const failCount = phase.agents.filter(a => a.status === 'failed').length
            return (
              <div key={phase.phase_id} className="rounded-lg border border-border bg-card">
                <div className="px-4 py-3 border-b border-border">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center gap-3">
                      <div className={cn('h-2.5 w-2.5 rounded-full', phaseDot[phase.status])} />
                      <span className="text-sm font-medium">
                        Phase {phase.phase_id} — {phase.label}
                      </span>
                      <span className={cn(
                        'rounded px-1.5 py-0.5 text-[10px] font-medium uppercase',
                        phase.role === 'producer' && 'bg-blue-500/12 text-blue-400',
                        phase.role === 'adversary' && 'bg-amber-500/12 text-amber-400',
                        phase.role === 'voter' && 'bg-purple-500/12 text-purple-400',
                      )}>
                        {phase.role}
                      </span>
                    </div>
                    {phase.agents.length > 0 && (
                      <span className="text-xs text-muted-foreground font-mono">
                        {okCount}/{phase.agents.length} done
                        {failCount > 0 && <span className="text-destructive"> · {failCount} failed</span>}
                      </span>
                    )}
                  </div>
                  {phase.description && (
                    <p className="mt-1.5 ml-6 text-xs text-muted-foreground">{phase.description}</p>
                  )}
                </div>
                <div className="p-4">
                  {phase.agents.length === 0 ? (
                    <p className="text-sm text-muted-foreground py-2">Pending...</p>
                  ) : (
                    <div className="grid gap-3 grid-cols-[repeat(auto-fill,minmax(240px,1fr))]">
                      {phase.agents.map((agent) => (
                        <AgentCard
                          key={agent.agent_id}
                          agent={agent}
                          onClick={() => openAgent(agent)}
                        />
                      ))}
                    </div>
                  )}
                </div>
              </div>
            )
          })}

          {/* Event Stream */}
          {events && events.length > 0 && <EventStream events={events} />}
        </div>
      </div>

      <DetailDrawer
        agent={selectedAgent}
        open={drawerOpen}
        onOpenChange={setDrawerOpen}
      />
    </div>
  )
}
