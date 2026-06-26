import { useQuery } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { mockApi } from '@/api/mock-client'
import type { RunCheckpoint } from '@/api/types'

export function useRunDetail(runId: string) {
  const query = useQuery({
    queryKey: ['run', runId],
    queryFn: () => mockApi.runs.get(runId),
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
  }, [runId, query.data?.status])

  return {
    ...query,
    data: liveCheckpoint ?? query.data,
  }
}

export function useRunEvents(runId: string) {
  return useQuery({
    queryKey: ['run-events', runId],
    queryFn: () => mockApi.runs.getEvents(runId),
    enabled: !!runId,
  })
}
