import type { LuftAPI } from './types-api'
import type {
  RunsResponse, RunCheckpoint, AgentEvent, DashboardStats,
  WorkflowSummary, WorkflowDetail, BackendConfig, RunFilters,
  StartRunRequest, StartRunResponse, CancelRunResponse,
  BackendTestResponse, RunLogsRequest, RunLogsResponse,
  RunArtifactsResponse, CreateBackendRequest, CreateBackendResponse,
  DeleteBackendResponse,
  WorkflowSaveRequest, WorkflowRunRequest,
} from './types'
import type { ApiHealth, ApiVersionInfo } from './common'
import {
  mockRuns, mockCheckpoints, mockEventsForRun, mockDashboardStats,
  mockWorkflows, mockWorkflowDetails, mockBackends,
  mockHealth, mockVersionInfo, mockLogsForRun, mockArtifactsForRun,
  mockCancelRun,
} from './mock-data'

function delay<T>(data: T, ms = 300): Promise<T> {
  return new Promise((resolve) => setTimeout(() => resolve(structuredClone(data)), ms))
}

export const mockApi: LuftAPI = {
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

    async cancel(runId: string): Promise<CancelRunResponse> {
      const run = mockRuns.find(r => r.run_id === runId)
      if (run) run.status = 'cancelled'
      return delay(mockCancelRun(runId))
    },

    async getLogs(req: RunLogsRequest): Promise<RunLogsResponse> {
      const result = mockLogsForRun(req.run_id)
      let lines = result.lines

      if (req.level) {
        const levels = ['debug', 'info', 'warn', 'error']
        const minIdx = levels.indexOf(req.level)
        lines = lines.filter(l => levels.indexOf(l.level) >= minIdx)
      }
      if (req.tail && req.tail > 0) {
        lines = lines.slice(-req.tail)
      }

      return delay({ ...result, lines })
    },

    async getArtifacts(runId: string): Promise<RunArtifactsResponse> {
      return delay(mockArtifactsForRun(runId))
    },
  },

  stats: {
    async get(): Promise<DashboardStats> {
      return delay(mockDashboardStats)
    },
  },

  system: {
    async health(): Promise<ApiHealth> {
      return delay(mockHealth)
    },

    async version(): Promise<ApiVersionInfo> {
      return delay(mockVersionInfo)
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
    async save(req: WorkflowSaveRequest): Promise<WorkflowDetail> {
      const existing = mockWorkflowDetails[req.name]
      const detail: WorkflowDetail = {
        name: req.name,
        content: req.content,
        description: req.description ?? existing?.description ?? '',
        last_modified: new Date().toISOString(),
      }
      mockWorkflowDetails[req.name] = detail
      if (!mockWorkflows.find(w => w.name === req.name)) {
        const phasesMatch = req.content.match(/phase\s*\(/g)
        const agentsMatch = req.content.match(/agent\s*\(/g)
        mockWorkflows.push({
          name: req.name,
          description: detail.description,
          phases: phasesMatch ? phasesMatch.length : 0,
          agents: agentsMatch ? agentsMatch.length : 0,
        })
      } else {
        const wf = mockWorkflows.find(w => w.name === req.name)!
        wf.description = detail.description
      }
      return delay(detail, 400)
    },
    async delete(name: string): Promise<void> {
      delete mockWorkflowDetails[name]
      const idx = mockWorkflows.findIndex(w => w.name === name)
      if (idx >= 0) mockWorkflows.splice(idx, 1)
      return delay(undefined, 300)
    },
    async run(req: WorkflowRunRequest): Promise<StartRunResponse> {
      const runId = `r4f2${Math.random().toString(36).slice(2, 8)}`
      return delay({
        run_id: runId,
        status: 'running',
        ws_url: `/ws/runs/${runId}`,
      }, 500)
    },
  },

  backends: {
    async list(): Promise<BackendConfig[]> {
      return delay(mockBackends)
    },

    async test(id: string): Promise<BackendTestResponse> {
      const backend = mockBackends.find(b => b.id === id)
      const connected = backend?.connected ?? false
      return delay({
        id,
        connected,
        latency_ms: connected ? 42 : undefined,
        error: connected ? undefined : 'Connection refused',
      }, 800)
    },

    async create(req: CreateBackendRequest): Promise<CreateBackendResponse> {
      const newId = `b${Date.now().toString(36)}`
      const backend: BackendConfig = {
        id: newId,
        name: req.name,
        provider: req.provider,
        model: req.model,
        connected: true,
        usage_count: 0,
      }
      return delay({ backend }, 600)
    },

    async delete(id: string): Promise<DeleteBackendResponse> {
      return delay({ id, deleted: true })
    },
  },
}
