import { FileText, FileCode, File, Download, Package } from 'lucide-react'
import { formatRelativeTime } from '@/lib/format'
import type { RunArtifact } from '@/api/types'

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function iconForMime(mime: string): typeof File {
  if (mime.startsWith('text/')) return FileText
  if (mime.includes('json') || mime.includes('javascript') || mime.includes('typescript')) return FileCode
  return File
}

interface RunArtifactsPanelProps {
  artifacts: RunArtifact[]
}

export function RunArtifactsPanel({ artifacts }: RunArtifactsPanelProps) {
  if (artifacts.length === 0) return null

  return (
    <div className="rounded-lg border border-border bg-card p-4">
      <div className="flex items-center gap-2 mb-3">
        <Package className="h-3.5 w-3.5 text-muted-foreground" />
        <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
          Artifacts
        </span>
        <span className="font-mono text-xs text-muted-foreground">({artifacts.length})</span>
      </div>
      <div className="space-y-1.5">
        {artifacts.map((artifact) => {
          const Icon = iconForMime(artifact.mime_type)
          return (
            <div
              key={artifact.path}
              className="group flex items-center gap-2.5 rounded-md px-2 py-1.5 hover:bg-hover/40 transition-colors"
            >
              <Icon className="h-4 w-4 shrink-0 text-muted-foreground group-hover:text-primary" />
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium truncate">{artifact.name}</div>
                <div className="text-xs text-muted-foreground flex items-center gap-2">
                  <span className="font-mono">{formatSize(artifact.size)}</span>
                  <span>·</span>
                  <span>{formatRelativeTime(artifact.created_at)}</span>
                </div>
              </div>
              <Download className="h-3.5 w-3.5 text-muted-foreground/40 opacity-0 group-hover:opacity-100 transition-opacity shrink-0" />
            </div>
          )
        })}
      </div>
    </div>
  )
}
