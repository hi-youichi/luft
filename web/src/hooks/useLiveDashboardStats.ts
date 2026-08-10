import { useQuery } from '@tanstack/react-query'
import { api, queryKeys } from '@/api'

export interface UseLiveDashboardStatsOptions {
  interval?: number
}

export function useLiveDashboardStats(options: UseLiveDashboardStatsOptions = {}) {
  const { interval = 10000 } = options

  return useQuery({
    queryKey: [...queryKeys.stats, 'live'] as const,
    queryFn: () => api.stats.get(),
    refetchInterval: (query) => {
      const active = query.state.data?.active_runs ?? []
      return active.length > 0 ? interval : false
    },
    refetchIntervalInBackground: false,
  })
}
