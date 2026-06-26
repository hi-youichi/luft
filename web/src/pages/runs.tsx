import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useRuns } from '@/hooks/useRuns'
import { StatusBadge } from '@/components/status-badge'
import { ProgressBar } from '@/components/progress-bar'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { formatTokens, formatElapsed, formatRelativeTime } from '@/lib/format'
import type { RunStatus } from '@/api/types'

export function RunsPage() {
  const [status, setStatus] = useState<RunStatus | 'all'>('all')
  const [time, setTime] = useState<'today' | '24h' | '7d' | 'all'>('all')
  const [query, setQuery] = useState('')
  const { data, isLoading } = useRuns({ status, time, q: query })
  const navigate = useNavigate()

  return (
    <div className="space-y-4">
      <h1 className="text-xl font-semibold font-display">Runs</h1>

      <div className="flex items-center gap-3">
        <Select value={status} onValueChange={(v) => setStatus(v as RunStatus | 'all')}>
          <SelectTrigger className="w-32"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="all">全部</SelectItem>
            <SelectItem value="running">运行中</SelectItem>
            <SelectItem value="completed">完成</SelectItem>
            <SelectItem value="failed">失败</SelectItem>
            <SelectItem value="cancelled">已取消</SelectItem>
          </SelectContent>
        </Select>
        <Select value={time} onValueChange={(v) => setTime(v as 'today' | '24h' | '7d' | 'all')}>
          <SelectTrigger className="w-32"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="today">今天</SelectItem>
            <SelectItem value="24h">24h</SelectItem>
            <SelectItem value="7d">7天</SelectItem>
            <SelectItem value="all">全部</SelectItem>
          </SelectContent>
        </Select>
        <Input
          placeholder="搜索 task..."
          className="max-w-xs"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {data && (
          <span className="ml-auto text-sm text-muted-foreground">{data.total} runs</span>
        )}
      </div>

      {isLoading ? (
        <div className="space-y-2">
          {Array.from({ length: 6 }).map((_, i) => (
            <Skeleton key={i} className="h-12" />
          ))}
        </div>
      ) : data && data.runs.length > 0 ? (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Task</TableHead>
              <TableHead className="w-28">Status</TableHead>
              <TableHead className="w-40">Phase</TableHead>
              <TableHead className="w-20 text-right">Tokens</TableHead>
              <TableHead className="w-24 text-right">Time</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {data.runs.map((run) => (
              <TableRow
                key={run.run_id}
                onClick={() => navigate(`/runs/${run.run_id}`)}
              >
                <TableCell className="font-medium max-w-md truncate">{run.task}</TableCell>
                <TableCell><StatusBadge status={run.status} /></TableCell>
                <TableCell>
                  <ProgressBar
                    current={run.current_phase}
                    total={run.total_phases}
                    showShimmer={run.status === 'running'}
                  />
                </TableCell>
                <TableCell className="text-right font-mono text-muted-foreground">
                  {formatTokens(run.total_tokens)}
                </TableCell>
                <TableCell className="text-right font-mono text-muted-foreground">
                  {run.status === 'running'
                    ? formatElapsed(run.elapsed_ms)
                    : formatRelativeTime(run.started_at)}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : (
        <div className="py-16 text-center text-muted-foreground">
          <p>No runs found</p>
        </div>
      )}
    </div>
  )
}
