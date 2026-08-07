import { apiConfig, type ApiMode } from './config'
import { getApi } from './api-client'

export type { ApiMode, ApiConfig } from './config'
export { apiConfig, updateApiConfig } from './config'

export { ApiError, NetworkError, TimeoutError } from './errors'

export {
  httpClient,
  type HttpMethod,
  type RequestOptions,
  type Interceptor,
} from './http-client'

export { apiClient, getApi } from './api-client'
export { mcpAdapter, resetMcpClient } from './mcp-adapter'
export { mockApi } from './mock-client'
export type {
  LuftAPI,
  RunAPI,
  StatsAPI,
  SystemAPI,
  WorkflowAPI,
  BackendAPI,
} from './types-api'

export { WSClient, type WSStatus, type WSClientOptions, type WSEvent, type WSMetrics, type IWSClient } from './ws-client'
export { createWSClient, createRunWSClient, isMockMode, buildWSUrl, buildRunWSUrl, httpToWs } from './ws-factory'

export {
  WSManager,
  getWSManager,
  resetWSManager,
  type WSManagerSubscription,
  type WSManagerMetrics,
  type WSManagerOptions,
  type MessageHandler,
  type StatusHandler,
} from './ws-manager'

export { queryKeys } from './query-keys'

export {
  setupApi,
  setupInterceptors,
  setAuthToken,
  setAuthTokenProvider,
  isApiError,
  type AuthTokenProvider,
} from './interceptors'

export * from './types'
export * from './common'
export * from './pagination'
export * from './endpoints'
export * from './ws-protocol'
export * from './type-guards'

export const api = getApi()

export function setApiMode(mode: ApiMode): void {
  apiConfig.mode = mode
}

export function getApiMode(): ApiMode {
  return apiConfig.mode
}
