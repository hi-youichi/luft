import { useRef, useEffect } from 'react'
import { Search, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { AgentRole, AgentStatus } from '@/api/types'

export type AgentFilterRole = AgentRole | 'all'
export type AgentFilterStatus = AgentStatus | 'all'

export interface AgentFilterState {
  query: string
  role: AgentFilterRole
  status: AgentFilterStatus
}

interface AgentFilterBarProps {
  filter: AgentFilterState
  onFilterChange: (filter: AgentFilterState) => void
  resultCount: number
  totalCount: number
  autoFocusKey?: string
}

const roleOptions: { value: AgentFilterRole; label: string; className: string }[] = [
  { value: 'all', label: 'All', className: 'text-muted-foreground' },
  { value: 'producer', label: 'Producer', className: 'text-blue-400' },
  { value: 'adversary', label: 'Adversary', className: 'text-amber-400' },
  { value: 'voter', label: 'Voter', className: 'text-purple-400' },
]

const statusOptions: { value: AgentFilterStatus; label: string; className: string }[] = [
  { value: 'all', label: 'All', className: 'text-muted-foreground' },
  { value: 'running', label: 'Running', className: 'text-blue-400' },
  { value: 'done', label: 'Done', className: 'text-primary' },
  { value: 'failed', label: 'Failed', className: 'text-destructive' },
  { value: 'pending', label: 'Pending', className: 'text-muted-foreground' },
]

export function AgentFilterBar({
  filter,
  onFilterChange,
  resultCount,
  totalCount,
  autoFocusKey,
}: AgentFilterBarProps) {
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (autoFocusKey) {
      inputRef.current?.focus()
    }
  }, [autoFocusKey])

  const isFiltered = filter.query !== '' || filter.role !== 'all' || filter.status !== 'all'

  return (
    <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-card px-3 py-2">
      <div className="relative flex-1 min-w-[180px]">
        <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground pointer-events-none" />
        <input
          ref={inputRef}
          type="text"
          value={filter.query}
          onChange={(e) => onFilterChange({ ...filter, query: e.target.value })}
          placeholder="Search agents..."
          className="w-full rounded-md border border-border bg-bg py-1.5 pl-8 pr-7 text-sm text-foreground placeholder:text-muted-foreground focus:border-primary/50 focus:outline-none focus:ring-1 focus:ring-primary/30 transition-colors"
        />
        {filter.query && (
          <button
            onClick={() => onFilterChange({ ...filter, query: '' })}
            className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        )}
      </div>

      <div className="flex items-center gap-1">
        {roleOptions.map((opt) => (
          <button
            key={opt.value}
            onClick={() => onFilterChange({ ...filter, role: opt.value })}
            className={cn(
              'rounded-md px-2 py-1 text-xs font-medium transition-colors',
              filter.role === opt.value
                ? 'bg-primary/10 text-primary'
                : cn('hover:bg-hover', opt.className),
            )}
          >
            {opt.label}
          </button>
        ))}
      </div>

      <div className="h-4 w-px bg-border" />

      <div className="flex items-center gap-1">
        {statusOptions.map((opt) => (
          <button
            key={opt.value}
            onClick={() => onFilterChange({ ...filter, status: opt.value })}
            className={cn(
              'rounded-md px-2 py-1 text-xs font-medium transition-colors',
              filter.status === opt.value
                ? 'bg-primary/10 text-primary'
                : cn('hover:bg-hover', opt.className),
            )}
          >
            {opt.label}
          </button>
        ))}
      </div>

      {isFiltered && (
        <div className="flex items-center gap-2 ml-auto">
          <span className="text-xs text-muted-foreground font-mono">
            {resultCount}/{totalCount}
          </span>
          <button
            onClick={() => onFilterChange({ query: '', role: 'all', status: 'all' })}
            className="text-xs text-primary hover:underline"
          >
            Clear
          </button>
        </div>
      )}
    </div>
  )
}
