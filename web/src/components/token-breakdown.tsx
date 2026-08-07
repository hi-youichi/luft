import { cn } from '@/lib/utils'
import { formatTokens } from '@/lib/format'
import type { TokenUsage } from '@/api/types'

interface TokenBreakdownProps {
  tokens: TokenUsage
  className?: string
}

export function TokenBreakdown({ tokens, className }: TokenBreakdownProps) {
  const total = tokens.input + tokens.output
  const inputPct = total > 0 ? (tokens.input / total) * 100 : 0
  const outputPct = total > 0 ? (tokens.output / total) * 100 : 0

  return (
    <div className={cn('space-y-1.5', className)}>
      <div className="flex h-1.5 w-full overflow-hidden rounded-full bg-muted">
        <div
          className="bg-blue-500/60 transition-all duration-300"
          style={{ width: `${inputPct}%` }}
        />
        <div
          className="bg-primary/60 transition-all duration-300"
          style={{ width: `${outputPct}%` }}
        />
      </div>
      <div className="flex items-center justify-between text-xs font-mono">
        <span className="flex items-center gap-1">
          <span className="h-2 w-2 rounded-full bg-blue-500/60" />
          <span className="text-muted-foreground">In</span>
          <span className="text-foreground">{formatTokens(tokens.input)}</span>
        </span>
        <span className="flex items-center gap-1">
          <span className="text-muted-foreground">Out</span>
          <span className="text-foreground">{formatTokens(tokens.output)}</span>
          <span className="h-2 w-2 rounded-full bg-primary/60" />
        </span>
      </div>
    </div>
  )
}
