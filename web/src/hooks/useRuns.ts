import { useQuery } from '@tanstack/react-query'
import { mockApi } from '@/api/mock-client'
import type { RunFilters } from '@/api/types'

export function useRuns(filters?: RunFilters) {
  return useQuery({
    queryKey: ['runs', filters],
    queryFn: () => mockApi.runs.list(filters),
  })
}
