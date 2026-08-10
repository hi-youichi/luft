import { useState } from 'react'

interface BarDatum {
  label: string
  value: number
  sublabel?: string
}

interface BarChartProps {
  data: BarDatum[]
  height?: number
  barColor?: string
  formatValue?: (v: number) => string
}

export function BarChart({
  data,
  height = 160,
  barColor = 'var(--color-primary)',
  formatValue = (v) => String(v),
}: BarChartProps) {
  const [hovered, setHovered] = useState<number | null>(null)
  const max = Math.max(...data.map((d) => d.value), 1)

  return (
    <div className="w-full">
      <div
        className="flex items-end justify-between gap-2"
        style={{ height }}
      >
        {data.map((d, i) => {
          const pct = (d.value / max) * 100
          const isHovered = hovered === i
          return (
            <div
              key={i}
              className="group relative flex flex-1 flex-col items-center justify-end"
              style={{ height: '100%' }}
              onMouseEnter={() => setHovered(i)}
              onMouseLeave={() => setHovered(null)}
            >
              {isHovered && d.value > 0 && (
                <div className="absolute -top-9 z-10 rounded-md border border-border bg-bg-elevated px-2 py-1 shadow-lg">
                  <span className="text-xs font-mono text-foreground">
                    {formatValue(d.value)}
                  </span>
                </div>
              )}
              <div
                className="w-full max-w-[2.5rem] rounded-t-sm transition-all duration-300"
                style={{
                  height: `${Math.max(pct, d.value > 0 ? 4 : 0)}%`,
                  backgroundColor: isHovered ? barColor : `${barColor}cc`,
                  opacity: d.value === 0 ? 0.2 : 1,
                }}
              />
            </div>
          )
        })}
      </div>
      <div className="mt-2 flex items-center justify-between gap-2">
        {data.map((d, i) => (
          <div
            key={i}
            className="flex flex-1 flex-col items-center text-center"
          >
            <span className="text-xs text-muted-foreground">{d.label}</span>
            {d.sublabel && (
              <span className="text-[10px] font-mono text-muted-foreground/60">
                {d.sublabel}
              </span>
            )}
          </div>
        ))}
      </div>
    </div>
  )
}
