import { useQuery } from '@tanstack/react-query'
import { api, queryKeys } from '@/api'
import type { RunLogsRequest } from '@/api/types'

export function useRunLogs(runId: string, req?: Omit<RunLogsRequest, 'run_id'>) {
  const fullReq: RunLogsRequest | undefined = req ? { run_id: runId, ...req } : undefined
  return useQuery({
    queryKey: queryKeys.runs.logs(runId, fullReq),
    queryFn: () => api.runs.getLogs({ run_id: runId, ...req }),
    enabled: !!runId,
    refetchInterval: (q) => {
      return q.state.data?.has_more ? 5000 : false
    },
  })
}
