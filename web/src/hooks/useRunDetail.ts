import { useQuery } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { api, queryKeys } from '@/api'
import type { RunCheckpoint } from '@/api/types'

export function useRunDetail(runId: string) {
  const query = useQuery({
    queryKey: queryKeys.runs.detail(runId),
    queryFn: () => api.runs.get(runId),
    enabled: !!runId,
  })

  const [liveCheckpoint, setLiveCheckpoint] = useState<RunCheckpoint | null>(null)

  useEffect(() => {
    if (query.data) {
      setLiveCheckpoint(query.data)
    }
  }, [query.data])

  useEffect(() => {
    if (!query.data || query.data.status !== 'running') return

    let checkpoint = structuredClone(query.data)

    const timer = setInterval(() => {
      let changed = false
      const cp = structuredClone(checkpoint)

      for (const phase of cp.phases) {
        for (const agent of phase.agents) {
          if (agent.status === 'running') {
            agent.tool_calls += 1
            agent.elapsed_ms += 3000
            changed = true
          }
        }
      }

      if (changed) {
        cp.elapsed_ms += 3000
        checkpoint = cp
        setLiveCheckpoint(cp)
      }
    }, 3000)

    return () => clearInterval(timer)
  }, [runId, query.data])

  return {
    ...query,
    data: liveCheckpoint ?? query.data,
  }
}

export function useRunEvents(runId: string) {
  return useQuery({
    queryKey: queryKeys.runs.events(runId),
    queryFn: () => api.runs.getEvents(runId),
    enabled: !!runId,
  })
}
