import type {
  RunsResponse,
  RunCheckpoint,
  AgentEvent,
  DashboardStats,
  WorkflowSummary,
  WorkflowDetail,
  BackendConfig,
  RunFilters,
  StartRunRequest,
  StartRunResponse,
  CancelRunResponse,
  BackendTestResponse,
  RunLogsRequest,
  RunLogsResponse,
  RunArtifactsResponse,
  CreateBackendRequest,
  CreateBackendResponse,
  DeleteBackendResponse,
} from './types'
import type { ApiHealth, ApiVersionInfo } from './common'
import type { PaginatedResponse } from './pagination'

export interface RunAPI {
  list(filters?: RunFilters): Promise<RunsResponse | PaginatedResponse<RunsResponse['runs'][number]>>
  get(runId: string): Promise<RunCheckpoint>
  getEvents(runId: string): Promise<AgentEvent[]>
  start(req: StartRunRequest): Promise<StartRunResponse>
  cancel(runId: string): Promise<CancelRunResponse>
  getLogs(req: RunLogsRequest): Promise<RunLogsResponse>
  getArtifacts(runId: string): Promise<RunArtifactsResponse>
}

export interface StatsAPI {
  get(): Promise<DashboardStats>
}

export interface SystemAPI {
  health(): Promise<ApiHealth>
  version(): Promise<ApiVersionInfo>
}

export interface WorkflowAPI {
  list(): Promise<WorkflowSummary[]>
  get(name: string): Promise<WorkflowDetail>
  save(req: import('./types').WorkflowSaveRequest): Promise<WorkflowDetail>
  delete(name: string): Promise<void>
  run(req: import('./types').WorkflowRunRequest): Promise<StartRunResponse>
}

export interface BackendAPI {
  list(): Promise<BackendConfig[]>
  test(id: string): Promise<BackendTestResponse>
  create(req: CreateBackendRequest): Promise<CreateBackendResponse>
  delete(id: string): Promise<DeleteBackendResponse>
}

export interface LuftAPI {
  runs: RunAPI
  stats: StatsAPI
  system: SystemAPI
  workflows: WorkflowAPI
  backends: BackendAPI
}
