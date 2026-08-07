import { useRef, useEffect, useState } from 'react'
import { ChevronDown, ChevronUp, Terminal } from 'lucide-react'
import { cn } from '@/lib/utils'
import { formatTime } from '@/lib/format'
import type { LogLine } from '@/api/types'

const levelStyles: Record<string, string> = {
  debug: 'text-muted-foreground/60',
  info: 'text-muted-foreground',
  warn: 'text-amber-400',
  error: 'text-destructive',
}

const levelBadge: Record<string, string> = {
  debug: 'bg-muted text-muted-foreground/60',
  info: 'bg-blue-500/12 text-blue-400',
  warn: 'bg-amber-500/12 text-amber-400',
  error: 'bg-destructive/12 text-destructive',
}

const severityOrder: Record<string, number> = {
  error: 0,
  warn: 1,
  info: 2,
  debug: 3,
}

interface RunLogsPanelProps {
  logs: LogLine[]
  hasMore?: boolean
}

export function RunLogsPanel({ logs, hasMore }: RunLogsPanelProps) {
  const [filter, setFilter] = useState<'all' | 'warn' | 'error'>('all')
  const [collapsed, setCollapsed] = useState(false)
  const scrollRef = useRef<HTMLDivElement>(null)

  const filtered = filter === 'all'
    ? logs
    : logs.filter((l) => severityOrder[l.level] <= severityOrder[filter])

  useEffect(() => {
    if (!collapsed && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight
    }
  }, [filtered, collapsed])

  return (
    <div className="rounded-lg border border-border bg-card">
      <div
        className="flex items-center justify-between px-4 py-3 cursor-pointer"
        onClick={() => setCollapsed((c) => !c)}
      >
        <div className="flex items-center gap-2">
          <Terminal className="h-3.5 w-3.5 text-muted-foreground" />
          <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
            Logs
          </span>
          {logs.length > 0 && (
            <span className="font-mono text-xs text-muted-foreground">({logs.length})</span>
          )}
        </div>
        <div className="flex items-center gap-2" onClick={(e) => e.stopPropagation()}>
          <div className="flex gap-1">
            {(['all', 'warn', 'error'] as const).map((f) => (
              <button
                key={f}
                onClick={() => setFilter(f)}
                className={cn(
                  'rounded px-2 py-0.5 text-[10px] font-medium uppercase transition-colors',
                  filter === f
                    ? 'bg-primary/12 text-primary'
                    : 'bg-muted text-muted-foreground hover:text-foreground',
                )}
              >
                {f}
              </button>
            ))}
          </div>
          {collapsed ? (
            <ChevronDown className="h-4 w-4 text-muted-foreground" />
          ) : (
            <ChevronUp className="h-4 w-4 text-muted-foreground" />
          )}
        </div>
      </div>
      {!collapsed && (
        <div
          ref={scrollRef}
          className="max-h-48 overflow-y-auto border-t border-border p-2 font-mono text-xs space-y-0.5"
        >
          {filtered.length === 0 ? (
            <div className="text-muted-foreground text-center py-4">
              {logs.length === 0 ? 'No logs available' : 'No logs match filter'}
            </div>
          ) : (
            filtered.map((line, i) => (
              <div key={i} className="flex items-start gap-2 px-2 py-0.5 rounded hover:bg-hover/30">
                <span className="text-muted-foreground/60 whitespace-nowrap shrink-0">
                  {formatTime(line.ts)}
                </span>
                <span
                  className={cn(
                    'shrink-0 rounded px-1 text-[9px] font-bold uppercase leading-4',
                    levelBadge[line.level],
                  )}
                >
                  {line.level}
                </span>
                {line.agent_id && (
                  <span className="text-blue-400/70 shrink-0">[{line.agent_id}]</span>
                )}
                <span className={cn('break-all', levelStyles[line.level])}>
                  {line.message}
                </span>
              </div>
            ))
          )}
          {hasMore && (
            <div className="text-center text-muted-foreground/50 text-[10px] py-1">
              More logs available...
            </div>
          )}
        </div>
      )}
    </div>
  )
}
