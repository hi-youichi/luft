import { httpClient } from './http-client'
import { endpoints } from './endpoints'
import { apiConfig } from './config'
import { mockApi } from './mock-client'
import { mcpAdapter } from './mcp-adapter'
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

function buildParams(filters?: RunFilters): Record<string, string | undefined> {
  const params: Record<string, string | undefined> = {}
  if (filters?.status && filters.status !== 'all') params.status = filters.status
  if (filters?.time && filters.time !== 'all') params.time = filters.time
  if (filters?.q) params.q = filters.q
  return params
}

function buildLogParams(req: RunLogsRequest): Record<string, string | number | undefined> {
  const params: Record<string, string | number | undefined> = {}
  if (req.tail) params.tail = req.tail
  if (req.since) params.since = req.since
  if (req.level) params.level = req.level
  return params
}

export const apiClient: LuftAPI = {
  runs: {
    list(filters?: RunFilters): Promise<RunsResponse> {
      return httpClient.get<RunsResponse>(endpoints.runs.list, { params: buildParams(filters) })
    },

    get(runId: string): Promise<RunCheckpoint> {
      return httpClient.get<RunCheckpoint>(endpoints.runs.get(runId))
    },

    getEvents(runId: string): Promise<AgentEvent[]> {
      return httpClient.get<AgentEvent[]>(endpoints.runs.events(runId))
    },

    start(req: StartRunRequest): Promise<StartRunResponse> {
      return httpClient.post<StartRunResponse>(endpoints.runs.start, req)
    },

    cancel(runId: string): Promise<CancelRunResponse> {
      return httpClient.post<CancelRunResponse>(endpoints.runs.cancel(runId))
    },

    getLogs(req: RunLogsRequest): Promise<RunLogsResponse> {
      return httpClient.get<RunLogsResponse>(endpoints.runs.logs(req.run_id), {
        params: buildLogParams(req),
      })
    },

    getArtifacts(runId: string): Promise<RunArtifactsResponse> {
      return httpClient.get<RunArtifactsResponse>(endpoints.runs.artifacts(runId))
    },
  },

  stats: {
    get(): Promise<DashboardStats> {
      return httpClient.get<DashboardStats>(endpoints.stats)
    },
  },

  system: {
    health(): Promise<ApiHealth> {
      return httpClient.get<ApiHealth>(endpoints.health)
    },

    version(): Promise<ApiVersionInfo> {
      return httpClient.get<ApiVersionInfo>(endpoints.version)
    },
  },

  workflows: {
    list(): Promise<WorkflowSummary[]> {
      return httpClient.get<WorkflowSummary[]>(endpoints.workflows.list)
    },

    get(name: string): Promise<WorkflowDetail> {
      return httpClient.get<WorkflowDetail>(endpoints.workflows.get(name))
    },

    save(req: WorkflowSaveRequest): Promise<WorkflowDetail> {
      return httpClient.put<WorkflowDetail>(endpoints.workflows.save(req.name), req)
    },

    delete(name: string): Promise<void> {
      return httpClient.delete<void>(endpoints.workflows.delete(name))
    },

    run(req: WorkflowRunRequest): Promise<StartRunResponse> {
      return httpClient.post<StartRunResponse>(endpoints.workflows.run(req.name), req)
    },
  },

  backends: {
    list(): Promise<BackendConfig[]> {
      return httpClient.get<BackendConfig[]>(endpoints.backends.list)
    },

    test(id: string): Promise<BackendTestResponse> {
      return httpClient.post<BackendTestResponse>(endpoints.backends.test(id))
    },

    create(req: CreateBackendRequest): Promise<CreateBackendResponse> {
      return httpClient.post<CreateBackendResponse>(endpoints.backends.create, req)
    },

    delete(id: string): Promise<DeleteBackendResponse> {
      return httpClient.delete<DeleteBackendResponse>(endpoints.backends.delete(id))
    },
  },
}

export function getApi(): LuftAPI {
  return apiConfig.mode === 'live' ? mcpAdapter : mockApi
}
