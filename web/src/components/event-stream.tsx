import { useRef, useEffect } from 'react'
import { Pause, Play } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useUIStore } from '@/stores/ui'
import { eventToDisplay } from '@/lib/event-utils'
import { formatTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { AgentEvent } from '@/api/types'

export function EventStream({ events }: { events: AgentEvent[] }) {
  const paused = useUIStore((s) => s.eventStreamPaused)
  const togglePause = useUIStore((s) => s.toggleEventPause)
  const scrollRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!paused && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight
    }
  }, [events, paused])

  const displays = events.map(eventToDisplay)

  return (
    <div className="border-t border-border">
      <div className="flex items-center justify-between px-4 py-2 border-b border-border">
        <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
          Event Stream
        </span>
        <Button variant="ghost" size="sm" onClick={togglePause} className="h-7">
          {paused ? <Play className="h-3 w-3" /> : <Pause className="h-3 w-3" />}
          {paused ? 'Resume' : 'Pause'}
        </Button>
      </div>
      <div ref={scrollRef} className="max-h-64 overflow-y-auto p-2 font-mono text-xs">
        {displays.length === 0 ? (
          <div className="text-muted-foreground text-center py-4">No events yet</div>
        ) : (
          displays.map((d, i) => (
            <div
              key={i}
              className={cn(
                'flex items-start gap-2 py-0.5 px-2 rounded hover:bg-hover/30',
                d.indent === 1 && 'ml-4',
                d.indent === 2 && 'ml-8',
              )}
            >
              <span className="text-muted-foreground whitespace-nowrap">{formatTime(d.ts)}</span>
              <span className={cn('select-none', d.iconColor)}>{d.icon}</span>
              <span className="text-foreground">{d.text}</span>
              {d.detail && <span className="text-muted-foreground">— {d.detail}</span>}
            </div>
          ))
        )}
      </div>
    </div>
  )
}
