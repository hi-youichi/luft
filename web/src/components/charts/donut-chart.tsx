interface DonutSegment {
  label: string
  value: number
  color: string
}

interface DonutChartProps {
  segments: DonutSegment[]
  size?: number
  thickness?: number
  centerLabel?: string
  centerValue?: string | number
}

export function DonutChart({
  segments,
  size = 180,
  thickness = 24,
  centerLabel,
  centerValue,
}: DonutChartProps) {
  const radius = (size - thickness) / 2
  const circumference = 2 * Math.PI * radius
  const total = segments.reduce((sum, s) => sum + s.value, 0)

  let offset = 0

  return (
    <div className="flex items-center gap-6">
      <div className="relative" style={{ width: size, height: size }}>
        <svg width={size} height={size} className="transform -rotate-90">
          <circle
            cx={size / 2}
            cy={size / 2}
            r={radius}
            fill="none"
            stroke="var(--color-bg-elevated)"
            strokeWidth={thickness}
          />
          {total > 0 &&
            segments.map((seg, i) => {
              const fraction = seg.value / total
              const dash = fraction * circumference
              const gap = circumference - dash
              const el = (
                <circle
                  key={i}
                  cx={size / 2}
                  cy={size / 2}
                  r={radius}
                  fill="none"
                  stroke={seg.color}
                  strokeWidth={thickness}
                  strokeDasharray={`${dash} ${gap}`}
                  strokeDashoffset={-offset}
                  strokeLinecap="butt"
                  className="transition-all duration-500"
                />
              )
              offset += dash
              return el
            })}
        </svg>
        {(centerLabel || centerValue !== undefined) && (
          <div className="absolute inset-0 flex flex-col items-center justify-center">
            {centerValue !== undefined && (
              <span className="text-2xl font-bold font-display text-foreground">
                {centerValue}
              </span>
            )}
            {centerLabel && (
              <span className="mt-0.5 text-xs text-muted-foreground">
                {centerLabel}
              </span>
            )}
          </div>
        )}
      </div>
      <div className="flex flex-col gap-2">
        {segments.map((seg, i) => (
          <div key={i} className="flex items-center gap-2">
            <div
              className="h-2.5 w-2.5 rounded-sm"
              style={{ backgroundColor: seg.color }}
            />
            <span className="text-sm text-foreground">{seg.label}</span>
            <span className="text-sm font-mono text-muted-foreground">
              {seg.value}
              {total > 0 && (
                <span className="ml-1 text-xs">
                  ({((seg.value / total) * 100).toFixed(0)}%)
                </span>
              )}
            </span>
          </div>
        ))}
      </div>
    </div>
  )
}
