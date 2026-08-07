import { useQuery } from '@tanstack/react-query'
import { api, queryKeys } from '@/api'
import type { RunFilters } from '@/api/types'

export interface UseLiveRunsOptions {
  filters?: RunFilters
  interval?: number
}

export function useLiveRuns(options: UseLiveRunsOptions = {}) {
  const { filters, interval = 5000 } = options

  return useQuery({
    queryKey: queryKeys.runs.live(filters),
    queryFn: () => api.runs.list(filters),
    refetchInterval: (query) => {
      const runs = query.state.data?.runs ?? []
      const hasActive = runs.some((r) => r.status === 'running')
      return hasActive ? interval : false
    },
    refetchIntervalInBackground: false,
  })
}
