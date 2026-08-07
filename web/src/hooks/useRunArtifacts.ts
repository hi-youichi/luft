import { useQuery } from '@tanstack/react-query'
import { api, queryKeys } from '@/api'

export function useRunArtifacts(runId: string) {
  return useQuery({
    queryKey: queryKeys.runs.artifacts(runId),
    queryFn: () => api.runs.getArtifacts(runId),
    enabled: !!runId,
  })
}
