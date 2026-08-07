import { useCallback, useEffect, useRef, useState } from 'react'
import { createWSClient } from '@/api/ws-factory'
import type { IWSClient, WSStatus } from '@/api/ws-client'
import type { AgentEvent } from '@/api/types'

export interface UseLiveEventsOptions {
  wsUrl?: string
  maxEvents?: number
  enabled?: boolean
}

export interface UseLiveEventsResult {
  events: AgentEvent[]
  status: WSStatus
  lastEvent: AgentEvent | null
  clear: () => void
}

export function useLiveEvents(
  runId: string,
  options: UseLiveEventsOptions = {},
): UseLiveEventsResult {
  const { wsUrl, maxEvents = 500, enabled = true } = options
  const [events, setEvents] = useState<AgentEvent[]>([])
  const [status, setStatus] = useState<WSStatus>('closed')
  const clientRef = useRef<IWSClient | null>(null)

  const url = wsUrl ?? `/ws/runs/${runId}`

  const clear = useCallback(() => setEvents([]), [])

  useEffect(() => {
    if (!enabled || !runId) return

    const client = createWSClient({
      url,
      onMessage: (event) => {
        setEvents((prev) => {
          const next = [...prev, event]
          return next.length > maxEvents ? next.slice(-maxEvents) : next
        })
      },
      onStatus: setStatus,
    })

    clientRef.current = client
    client.connect()

    return () => {
      client.close()
      clientRef.current = null
    }
  }, [runId, url, enabled, maxEvents])

  return {
    events,
    status,
    lastEvent: events.length > 0 ? events[events.length - 1] : null,
    clear,
  }
}
