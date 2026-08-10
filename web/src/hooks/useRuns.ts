import { useQuery } from '@tanstack/react-query'
import { api, queryKeys } from '@/api'
import type { RunFilters } from '@/api/types'

export function useRuns(filters?: RunFilters) {
  return useQuery({
    queryKey: queryKeys.runs.list(filters),
    queryFn: () => api.runs.list(filters),
  })
}
