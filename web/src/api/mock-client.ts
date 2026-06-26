import type {
  RunsResponse, RunCheckpoint, AgentEvent, DashboardStats,
  WorkflowSummary, WorkflowDetail, BackendConfig, RunFilters,
  StartRunRequest, StartRunResponse,
} from './types'
import {
  mockRuns, mockCheckpoints, mockEventsForRun, mockDashboardStats,
  mockWorkflows, mockWorkflowDetails, mockBackends,
} from './mock-data'

function delay<T>(data: T, ms = 300): Promise<T> {
  return new Promise((resolve) => setTimeout(() => resolve(structuredClone(data)), ms))
}

export const mockApi = {
  runs: {
    async list(filters?: RunFilters): Promise<RunsResponse> {
      let runs = [...mockRuns]

      if (filters?.status && filters.status !== 'all') {
        runs = runs.filter(r => r.status === filters.status)
      }
      if (filters?.q) {
        const q = filters.q.toLowerCase()
        runs = runs.filter(r => r.task.toLowerCase().includes(q))
      }
      if (filters?.time && filters.time !== 'all') {
        const cutoff = Date.now()
        const ranges: Record<string, number> = { today: 24, '24h': 24, '7d': 168 }
        const hours = ranges[filters.time] ?? Infinity
        runs = runs.filter(r => cutoff - new Date(r.started_at).getTime() < hours * 3600_000)
      }

      return delay({ runs, total: runs.length })
    },

    async get(runId: string): Promise<RunCheckpoint> {
      const cp = mockCheckpoints[runId]
      if (!cp) return Promise.reject(new Error(`Run ${runId} not found`))
      return delay(cp)
    },

    async getEvents(runId: string): Promise<AgentEvent[]> {
      return delay(mockEventsForRun(runId))
    },

    async start(_req: StartRunRequest): Promise<StartRunResponse> {
      const runId = `r4f2${Math.random().toString(36).slice(2, 8)}`
      return delay({
        run_id: runId,
        status: 'running',
        ws_url: `/ws/runs/${runId}`,
      }, 500)
    },
  },

  stats: {
    async get(): Promise<DashboardStats> {
      return delay(mockDashboardStats)
    },
  },

  workflows: {
    async list(): Promise<WorkflowSummary[]> {
      return delay(mockWorkflows)
    },
    async get(name: string): Promise<WorkflowDetail> {
      const wf = mockWorkflowDetails[name]
      if (!wf) return Promise.reject(new Error(`Workflow ${name} not found`))
      return delay(wf)
    },
  },

  backends: {
    async list(): Promise<BackendConfig[]> {
      return delay(mockBackends)
    },
  },
}
