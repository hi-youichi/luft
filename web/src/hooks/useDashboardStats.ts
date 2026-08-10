import { useQuery } from '@tanstack/react-query'
import { api, queryKeys } from '@/api'

export function useDashboardStats() {
  return useQuery({
    queryKey: queryKeys.stats,
    queryFn: () => api.stats.get(),
  })
}
