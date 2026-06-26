import { useQuery } from '@tanstack/react-query'
import { mockApi } from '@/api/mock-client'

export function useDashboardStats() {
  return useQuery({
    queryKey: ['dashboard-stats'],
    queryFn: () => mockApi.stats.get(),
  })
}
