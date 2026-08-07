import { useState, useMemo } from 'react'
import { cn } from '@/lib/utils'
import { formatTokens } from '@/lib/format'
import type { PhaseDetail } from '@/api/types'

interface PhaseTokenChartProps {
  phases: PhaseDetail[]
  className?: string
}

type ChartMode = 'total' | 'input' | 'output'

const modeColors: Record<ChartMode, string> = {
  total: 'var(--color-primary)',
  input: 'rgb(59 130 246 / 0.7)',
  output: 'var(--color-primary)',
}

export function PhaseTokenChart({ phases, className }: PhaseTokenChartProps) {
  const [mode, setMode] = useState<ChartMode>('total')

  const data = useMemo(() => {
    return phases.map((p) => {
      const input = p.agents.reduce((s, a) => s + a.tokens.input, 0)
      const output = p.agents.reduce((s, a) => s + a.tokens.output, 0)
      const total = input + output
      return {
        label: `P${p.phase_id}`,
        sublabel: p.label.slice(0, 12),
        input,
        output,
        total,
        value: mode === 'total' ? total : mode === 'input' ? input : output,
      }
    })
  }, [phases, mode])

  const max = Math.max(...data.map((d) => d.value), 1)

  return (
    <div className={cn('rounded-lg border border-border bg-card p-4', className)}>
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
          Token Distribution
        </span>
        <div className="flex items-center gap-1">
          {(['total', 'input', 'output'] as ChartMode[]).map((m) => (
            <button
              key={m}
              onClick={() => setMode(m)}
              className={cn(
                'rounded-md px-2 py-0.5 text-[10px] font-medium uppercase transition-colors',
                mode === m
                  ? 'bg-primary/10 text-primary'
                  : 'text-muted-foreground hover:text-foreground hover:bg-hover',
              )}
            >
              {m}
            </button>
          ))}
        </div>
      </div>

      {data.every((d) => d.value === 0) ? (
        <div className="flex items-center justify-center h-32 text-sm text-muted-foreground">
          No token data yet
        </div>
      ) : (
        <div className="space-y-2">
          <div className="flex items-end justify-between gap-2 h-32">
            {data.map((d, i) => {
              const pct = (d.value / max) * 100
              return (
                <div
                  key={i}
                  className="group relative flex flex-1 flex-col items-center justify-end"
                  style={{ height: '100%' }}
                >
                  <div className="absolute -top-7 z-10 hidden group-hover:block rounded-md border border-border bg-bg-elevated px-2 py-1 shadow-lg whitespace-nowrap">
                    <div className="text-xs font-mono text-foreground">
                      {formatTokens(d.value)} tokens
                    </div>
                    <div className="text-[10px] text-muted-foreground">
                      {formatTokens(d.input)} in · {formatTokens(d.output)} out
                    </div>
                  </div>
                  <div
                    className="w-full max-w-[2rem] rounded-t-sm transition-all duration-300"
                    style={{
                      height: `${Math.max(pct, d.value > 0 ? 4 : 0)}%`,
                      backgroundColor: modeColors[mode],
                      opacity: d.value === 0 ? 0.2 : 1,
                    }}
                  />
                </div>
              )
            })}
          </div>
          <div className="flex items-center justify-between gap-2">
            {data.map((d, i) => (
              <div key={i} className="flex flex-1 flex-col items-center text-center">
                <span className="text-xs text-muted-foreground">{d.label}</span>
                <span className="text-[10px] font-mono text-muted-foreground/60 truncate max-w-[4rem]">
                  {d.sublabel}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
