import type { HttpMethod } from './http-client'
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
  WorkflowSaveRequest,
  WorkflowRunRequest,
  RunId,
} from './types'
import type { ApiHealth, ApiVersionInfo, AckResponse } from './common'
import type { PaginatedResponse } from './pagination'

export interface EndpointDef<
  M extends HttpMethod,
  Req = undefined,
  Res = unknown,
  Q = undefined,
> {
  method: M
  path: string
  query?: Q
  request?: Req
  response: Res
}

export type EndpointParams<T extends string> =
  T extends `${string}{${infer Param}}/${infer Rest}`
    ? Param | EndpointParams<Rest>
    : T extends `${string}{${infer Param}}`
      ? Param
      : never

export interface EndpointMap {
  'GET /stats': EndpointDef<'GET', undefined, DashboardStats>
  'GET /health': EndpointDef<'GET', undefined, ApiHealth>
  'GET /version': EndpointDef<'GET', undefined, ApiVersionInfo>

  'GET /runs': EndpointDef<'GET', undefined, RunsResponse | PaginatedResponse<RunsResponse['runs'][number]>, RunFilters>
  'GET /runs/{runId}': EndpointDef<'GET', undefined, RunCheckpoint, undefined>
  'GET /runs/{runId}/events': EndpointDef<'GET', undefined, AgentEvent[], undefined>
  'POST /runs': EndpointDef<'POST', StartRunRequest, StartRunResponse>
  'POST /runs/{runId}/cancel': EndpointDef<'POST', undefined, CancelRunResponse>

  'GET /workflows': EndpointDef<'GET', undefined, WorkflowSummary[]>
  'GET /workflows/{name}': EndpointDef<'GET', undefined, WorkflowDetail>
  'PUT /workflows/{name}': EndpointDef<'PUT', WorkflowSaveRequest, WorkflowDetail>
  'DELETE /workflows/{name}': EndpointDef<'DELETE', undefined, void>
  'POST /workflows/{name}/run': EndpointDef<'POST', WorkflowRunRequest, StartRunResponse>

  'GET /backends': EndpointDef<'GET', undefined, BackendConfig[]>
  'POST /backends': EndpointDef<'POST', CreateBackendRequest, CreateBackendResponse>
  'POST /backends/{id}/test': EndpointDef<'POST', undefined, BackendTestResponse>
  'DELETE /backends/{id}': EndpointDef<'DELETE', undefined, DeleteBackendResponse>

  'GET /runs/{runId}/logs': EndpointDef<'GET', undefined, RunLogsResponse, RunLogsRequest>
  'GET /runs/{runId}/artifacts': EndpointDef<'GET', undefined, RunArtifactsResponse, undefined>
}

export type EndpointKey = keyof EndpointMap

export type EndpointResponse<K extends EndpointKey> = EndpointMap[K]['response']
export type EndpointRequest<K extends EndpointKey> = EndpointMap[K]['request']
export type EndpointQuery<K extends EndpointKey> = EndpointMap[K]['query']
export type EndpointMethod<K extends EndpointKey> = EndpointMap[K]['method']

const RUN_PREFIX = '/runs'
const WORKFLOW_PREFIX = '/workflows'
const BACKEND_PREFIX = '/backends'

export const endpoints = {
  stats: `${RUN_PREFIX.replace('/runs', '')}/stats`,
  health: '/health',
  version: '/version',

  runs: {
    list: RUN_PREFIX,
    get: (runId: RunId) => `${RUN_PREFIX}/${runId}`,
    events: (runId: RunId) => `${RUN_PREFIX}/${runId}/events`,
    logs: (runId: RunId) => `${RUN_PREFIX}/${runId}/logs`,
    artifacts: (runId: RunId) => `${RUN_PREFIX}/${runId}/artifacts`,
    start: RUN_PREFIX,
    cancel: (runId: RunId) => `${RUN_PREFIX}/${runId}/cancel`,
  },

  workflows: {
    list: WORKFLOW_PREFIX,
    get: (name: string) => `${WORKFLOW_PREFIX}/${name}`,
    save: (name: string) => `${WORKFLOW_PREFIX}/${name}`,
    delete: (name: string) => `${WORKFLOW_PREFIX}/${name}`,
    run: (name: string) => `${WORKFLOW_PREFIX}/${name}/run`,
  },

  backends: {
    list: BACKEND_PREFIX,
    create: BACKEND_PREFIX,
    test: (id: string) => `${BACKEND_PREFIX}/${id}/test`,
    delete: (id: string) => `${BACKEND_PREFIX}/${id}`,
  },
} as const

export type Endpoints = typeof endpoints

export function buildEndpointPath(template: string, params: Record<string, string>): string {
  return template.replace(/\{(\w+)\}/g, (_, key: string) =>
    params[key] ?? `{${key}}`,
  )
}

export type { AckResponse }
