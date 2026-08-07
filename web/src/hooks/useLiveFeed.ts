import { useEffect, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { api, queryKeys } from '@/api'
import { createWSClient } from '@/api/ws-factory'
import type { IWSClient, WSStatus } from '@/api/ws-client'
import type { AgentEvent, RunSummary } from '@/api/types'

const MAX_EVENTS = 200

export interface UseLiveFeedResult {
  runs: RunSummary[]
  events: AgentEvent[]
  wsTotal: number
  wsConnected: number
  isLoading: boolean
}

export function useLiveFeed(): UseLiveFeedResult {
  const { data, isLoading } = useQuery({
    queryKey: queryKeys.runs.live({ status: 'running' }),
    queryFn: () => api.runs.list({ status: 'running' }),
    refetchInterval: 5000,
  })

  const runs = data?.runs ?? []
  const [events, setEvents] = useState<AgentEvent[]>([])
  const [statuses, setStatuses] = useState<Record<string, WSStatus>>({})
  const connectionsRef = useRef<Map<string, IWSClient>>(new Map())

  useEffect(() => {
    const currentIds = new Set(runs.map((r) => r.run_id))

    for (const [runId, client] of connectionsRef.current) {
      if (!currentIds.has(runId)) {
        client.close()
        connectionsRef.current.delete(runId)
        setStatuses((prev) => {
          const next = { ...prev }
          delete next[runId]
          return next
        })
      }
    }

    for (const run of runs) {
      if (!connectionsRef.current.has(run.run_id)) {
        const client = createWSClient({
          url: `/ws/runs/${run.run_id}`,
          onMessage: (event) => {
            setEvents((prev) => {
              const next = [...prev, event]
              return next.length > MAX_EVENTS ? next.slice(-MAX_EVENTS) : next
            })
          },
          onStatus: (status) => {
            setStatuses((prev) => ({ ...prev, [run.run_id]: status }))
          },
        })
        connectionsRef.current.set(run.run_id, client)
        client.connect()
      }
    }
  }, [runs])

  useEffect(() => {
    return () => {
      for (const client of connectionsRef.current.values()) {
        client.close()
      }
      connectionsRef.current.clear()
    }
  }, [])

  const wsConnected = Object.values(statuses).filter((s) => s === 'open').length

  return {
    runs,
    events,
    wsTotal: connectionsRef.current.size,
    wsConnected,
    isLoading,
  }
}
