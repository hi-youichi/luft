import { useState, useMemo, useCallback } from 'react'
import { useParams, Link } from 'react-router-dom'
import { ArrowLeft, ChevronRight, Ban, Rows3, Columns3, BarChart3, Keyboard } from 'lucide-react'
import { useLiveRunDetail } from '@/hooks/useLiveRunDetail'
import { useRunLogs } from '@/hooks/useRunLogs'
import { useRunArtifacts } from '@/hooks/useRunArtifacts'
import { useCancelRun } from '@/hooks/useCancelRun'
import { useKeyboardShortcuts } from '@/hooks/useKeyboardShortcuts'
import { StatusBadge } from '@/components/status-badge'
import { ProgressBar } from '@/components/progress-bar'
import { DetailDrawer } from '@/components/detail-drawer'
import { EventStream } from '@/components/event-stream'
import { Timeline } from '@/components/timeline'
import { PhaseAccordion } from '@/components/phase-accordion'
import { TokenBreakdown } from '@/components/token-breakdown'
import { RunLogsPanel } from '@/components/run-logs-panel'
import { RunArtifactsPanel } from '@/components/run-artifacts-panel'
import { AgentFilterBar, type AgentFilterState } from '@/components/agent-filter-bar'
import { RunMetricsSummary } from '@/components/run-metrics-summary'
import { PhaseTokenChart } from '@/components/phase-token-chart'
import { RunExportMenu } from '@/components/run-export-menu'
import { Skeleton } from '@/components/ui/skeleton'
import { Button } from '@/components/ui/button'
import { useUIStore } from '@/stores/ui'
import { formatTokens, formatElapsed, formatRelativeTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { AgentResultCache, PhaseDetail } from '@/api/types'

function filterAgentsInPhase(
  phase: PhaseDetail,
  filter: AgentFilterState,
): PhaseDetail {
  if (filter.query === '' && filter.role === 'all' && filter.status === 'all') {
    return phase
  }
  return {
    ...phase,
    agents: phase.agents.filter((a) => {
      if (filter.role !== 'all' && a.role !== filter.role) return false
      if (filter.status !== 'all' && a.status !== filter.status) return false
      if (filter.query && !a.agent_id.toLowerCase().includes(filter.query.toLowerCase())) return false
      return true
    }),
  }
}

export function RunDetailPage() {
  const { runId } = useParams<{ runId: string }>()
  const { data: run, events, isLoading } = useLiveRunDetail(runId!)
  const { data: logsData } = useRunLogs(runId!)
  const { data: artifactsData } = useRunArtifacts(runId!)
  const cancelRun = useCancelRun()

  const [selectedAgent, setSelectedAgent] = useState<AgentResultCache | null>(null)
  const [drawerOpen, setDrawerOpen] = useState(false)
  const [agentFilter, setAgentFilter] = useState<AgentFilterState>({ query: '', role: 'all', status: 'all' })
  const [searchFocusKey, setSearchFocusKey] = useState(0)
  const [showShortcuts, setShowShortcuts] = useState(false)

  const selectedPhaseId = useUIStore((s) => s.selectedPhaseId)
  const setSelectedPhase = useUIStore((s) => s.setSelectedPhase)
  const density = useUIStore((s) => s.agentCardDensity)
  const toggleDensity = useUIStore((s) => s.toggleDensity)
  const toggleEventPause = useUIStore((s) => s.toggleEventPause)
  const showMetrics = useUIStore((s) => s.runDetailShowMetrics)
  const setShowMetrics = useUIStore((s) => s.setRunDetailShowMetrics)

  const allAgents = useMemo(
    () => run?.phases.flatMap((p) => p.agents) ?? [],
    [run],
  )
  const totalAgentCount = allAgents.length

  const filteredAgents = useMemo(
    () => allAgents.filter((a) => {
      if (agentFilter.role !== 'all' && a.role !== agentFilter.role) return false
      if (agentFilter.status !== 'all' && a.status !== agentFilter.status) return false
      if (agentFilter.query && !a.agent_id.toLowerCase().includes(agentFilter.query.toLowerCase())) return false
      return true
    }),
    [allAgents, agentFilter],
  )

  const openAgent = useCallback((agent: AgentResultCache) => {
    setSelectedAgent(agent)
    setDrawerOpen(true)
  }, [])

  useKeyboardShortcuts({
    onSearch: () => setSearchFocusKey((k) => k + 1),
    onClose: () => {
      if (drawerOpen) setDrawerOpen(false)
    },
    onToggleDensity: toggleDensity,
    onTogglePause: toggleEventPause,
  })

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

  const findingsCount = run.findings.length

  const visiblePhases: PhaseDetail[] = (selectedPhaseId != null
    ? run.phases.filter((p) => p.phase_id === selectedPhaseId)
    : run.phases
  ).map((p) => filterAgentsInPhase(p, agentFilter))

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <Button variant="ghost" size="sm" asChild>
          <Link to="/runs"><ArrowLeft className="h-3.5 w-3.5" /> 返回列表</Link>
        </Button>
        <ChevronRight className="h-3 w-3" />
        <span className="text-foreground font-mono">#{runId}</span>
      </div>

      <div className="rounded-lg border border-border bg-card p-5">
        <div className="flex items-start justify-between mb-3">
          <div className="min-w-0 flex-1">
            <h1 className="text-xl font-semibold font-display truncate">{run.task}</h1>
            <div className="mt-1 flex items-center gap-3 text-xs text-muted-foreground">
              <span>{formatRelativeTime(run.started_at)}</span>
              <span>·</span>
              <span>{formatElapsed(run.elapsed_ms)}</span>
              <span>·</span>
              <span>{run.phases.length} phases</span>
              <span>·</span>
              <span>{totalAgentCount} agents</span>
            </div>
          </div>
          <div className="flex items-center gap-3 shrink-0">
            <div className="text-right">
              <div className="font-mono text-lg font-semibold text-foreground">
                {formatTokens(run.total_tokens)}
              </div>
              <div className="text-xs text-muted-foreground">tokens</div>
            </div>
            {run.status === 'running' && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => cancelRun.mutate(runId!)}
                disabled={cancelRun.isPending}
                className="text-destructive hover:text-destructive"
              >
                <Ban className="h-3.5 w-3.5" />
                Cancel
              </Button>
            )}
            <RunExportMenu run={run} events={events} />
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setShowMetrics(!showMetrics)}
              title="Toggle metrics"
              className={cn(showMetrics && 'text-primary')}
            >
              <BarChart3 className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={toggleDensity}
              title={`Density: ${density}`}
            >
              {density === 'compact' ? (
                <Rows3 className="h-3.5 w-3.5" />
              ) : (
                <Columns3 className="h-3.5 w-3.5" />
              )}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setShowShortcuts((s) => !s)}
              title="Keyboard shortcuts"
            >
              <Keyboard className="h-3.5 w-3.5" />
            </Button>
            <StatusBadge status={run.status} />
          </div>
        </div>
        <div className="flex items-center justify-between gap-6">
          <ProgressBar
            current={run.current_phase}
            total={run.phases.length}
            showShimmer={run.status === 'running'}
            className="text-sm"
          />
          <TokenBreakdown tokens={run.total_tokens_detail} className="w-56" />
        </div>
      </div>

      {showShortcuts && (
        <div className="rounded-lg border border-border bg-card p-3">
          <div className="flex flex-wrap gap-4 text-xs text-muted-foreground">
            <span><kbd className="rounded border border-border px-1.5 py-0.5 font-mono">/</kbd> Focus search</span>
            <span><kbd className="rounded border border-border px-1.5 py-0.5 font-mono">j</kbd> / <kbd className="rounded border border-border px-1.5 py-0.5 font-mono">k</kbd> Navigate</span>
            <span><kbd className="rounded border border-border px-1.5 py-0.5 font-mono">d</kbd> Toggle density</span>
            <span><kbd className="rounded border border-border px-1.5 py-0.5 font-mono">p</kbd> Pause events</span>
            <span><kbd className="rounded border border-border px-1.5 py-0.5 font-mono">Esc</kbd> Close drawer</span>
          </div>
        </div>
      )}

      {showMetrics && <RunMetricsSummary run={run} />}

      <PhaseTokenChart phases={run.phases} />

      <AgentFilterBar
        filter={agentFilter}
        onFilterChange={setAgentFilter}
        resultCount={filteredAgents.length}
        totalCount={totalAgentCount}
        autoFocusKey={searchFocusKey ? String(searchFocusKey) : undefined}
      />

      <div className="flex gap-4">
        <aside className="w-52 shrink-0 space-y-4">
          <div className="rounded-lg border border-border bg-card p-4">
            <div className="flex items-center justify-between mb-3">
              <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
                Timeline
              </span>
              {selectedPhaseId != null && (
                <button
                  onClick={() => setSelectedPhase(null)}
                  className="text-[10px] text-primary hover:underline"
                >
                  Show all
                </button>
              )}
            </div>
            <Timeline
              phases={run.phases}
              selectedPhaseId={selectedPhaseId}
              onSelectPhase={(id) => setSelectedPhase(selectedPhaseId === id ? null : id)}
            />
          </div>

          {findingsCount > 0 && (
            <div className="rounded-lg border border-border bg-card p-4">
              <div className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                Findings
              </div>
              <div className="text-2xl font-bold font-display text-foreground">{findingsCount}</div>
              <div className="mt-2 space-y-1">
                {['critical', 'high', 'medium', 'low'].map((sev) => {
                  const count = run.findings.filter((f) => f.severity === sev).length
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

          {artifactsData && artifactsData.artifacts.length > 0 && (
            <RunArtifactsPanel artifacts={artifactsData.artifacts} />
          )}
        </aside>

        <div className="flex-1 min-w-0 space-y-2">
          {visiblePhases.map((phase) => (
            <PhaseAccordion
              key={phase.phase_id}
              phase={phase}
              defaultOpen={selectedPhaseId === phase.phase_id || (selectedPhaseId == null && phase.status !== 'completed')}
              onAgentClick={openAgent}
            />
          ))}

          {logsData && logsData.lines.length > 0 && (
            <RunLogsPanel logs={logsData.lines} hasMore={logsData.has_more} />
          )}

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
