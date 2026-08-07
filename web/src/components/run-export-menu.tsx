import { Download, FileJson, FileText, ChevronDown } from 'lucide-react'
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from '@/components/ui/dropdown-menu'
import { Button } from '@/components/ui/button'
import { formatTokens, formatElapsed, formatTime } from '@/lib/format'
import type { RunCheckpoint } from '@/api/types'

interface RunExportMenuProps {
  run: RunCheckpoint
  events?: unknown[]
}

function downloadBlob(content: string, filename: string, mime: string) {
  const blob = new Blob([content], { type: mime })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}

function exportJSON(run: RunCheckpoint, events?: unknown[]) {
  const payload = {
    run_id: run.run_id,
    task: run.task,
    status: run.status,
    started_at: run.started_at,
    elapsed_ms: run.elapsed_ms,
    total_tokens: run.total_tokens,
    total_tokens_detail: run.total_tokens_detail,
    phases: run.phases,
    findings: run.findings,
    events: events ?? [],
    exported_at: new Date().toISOString(),
  }
  downloadBlob(JSON.stringify(payload, null, 2), `run-${run.run_id}.json`, 'application/json')
}

function exportMarkdown(run: RunCheckpoint) {
  const lines: string[] = []
  lines.push(`# Run ${run.run_id}`)
  lines.push('')
  lines.push(`- **Task:** ${run.task}`)
  lines.push(`- **Status:** ${run.status}`)
  lines.push(`- **Started:** ${formatTime(run.started_at)}`)
  lines.push(`- **Elapsed:** ${formatElapsed(run.elapsed_ms)}`)
  lines.push(`- **Total Tokens:** ${formatTokens(run.total_tokens)} (${run.total_tokens_detail.input} in / ${run.total_tokens_detail.output} out)`)
  lines.push('')

  lines.push('## Phases')
  lines.push('')
  for (const phase of run.phases) {
    const done = phase.agents.filter((a) => a.status === 'done').length
    const failed = phase.agents.filter((a) => a.status === 'failed').length
    lines.push(`### Phase ${phase.phase_id}: ${phase.label}`)
    lines.push(`- Role: ${phase.role}`)
    lines.push(`- Status: ${phase.status}`)
    lines.push(`- Agents: ${phase.agents.length} (${done} done, ${failed} failed)`)
    lines.push('')

    if (phase.agents.length > 0) {
      lines.push('| Agent | Role | Status | Tokens | Elapsed |')
      lines.push('|-------|------|--------|--------|---------|')
      for (const a of phase.agents) {
        const tok = formatTokens(a.tokens.input + a.tokens.output)
        const el = a.elapsed_ms > 0 ? formatElapsed(a.elapsed_ms) : '—'
        lines.push(`| ${a.agent_id} | ${a.role} | ${a.status} | ${tok} | ${el} |`)
      }
      lines.push('')
    }
  }

  if (run.findings.length > 0) {
    lines.push('## Findings')
    lines.push('')
    for (const f of run.findings) {
      lines.push(`- **[${f.severity.toUpperCase()}]** ${f.message}${f.source ? ` _(source: ${f.source})_` : ''}`)
    }
    lines.push('')
  }

  downloadBlob(lines.join('\n'), `run-${run.run_id}.md`, 'text/markdown')
}

export function RunExportMenu({ run, events }: RunExportMenuProps) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="sm">
          <Download className="h-3.5 w-3.5" />
          Export
          <ChevronDown className="h-3 w-3 ml-0.5" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-40">
        <DropdownMenuLabel>Export Run Details</DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={() => exportJSON(run, events)}>
          <FileJson className="h-3.5 w-3.5" />
          <span>JSON</span>
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => exportMarkdown(run)}>
          <FileText className="h-3.5 w-3.5" />
          <span>Markdown</span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
