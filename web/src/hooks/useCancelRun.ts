import { useMutation, useQueryClient } from '@tanstack/react-query'
import { api, queryKeys } from '@/api'

export function useCancelRun() {
  const qc = useQueryClient()

  return useMutation({
    mutationFn: (runId: string) => api.runs.cancel(runId),
    onSuccess: (_data, runId) => {
      qc.invalidateQueries({ queryKey: queryKeys.runs.detail(runId) })
      qc.invalidateQueries({ queryKey: queryKeys.runs.all })
    },
  })
}
