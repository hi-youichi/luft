import type { RunId } from './types'

export type SortOrder = 'asc' | 'desc'

export type SortField<T> = {
  field: keyof T
  order: SortOrder
}

export interface ApiSuccessEnvelope<T> {
  data: T
  meta?: ResponseMeta
}

export interface ApiErrorEnvelope {
  error: {
    code: string
    message: string
    details?: Record<string, unknown>
  }
  meta?: ResponseMeta
}

export type ApiResponse<T> = ApiSuccessEnvelope<T> | ApiErrorEnvelope

export interface ResponseMeta {
  request_id?: string
  timestamp?: string
  duration_ms?: number
  api_version?: string
}

export function isApiError(resp: ApiResponse<unknown>): resp is ApiErrorEnvelope {
  return 'error' in resp
}

export function isApiSuccess<T>(resp: ApiResponse<T>): resp is ApiSuccessEnvelope<T> {
  return 'data' in resp
}

export function unwrapResponse<T>(resp: ApiResponse<T>): T {
  if (isApiError(resp)) {
    throw new Error(resp.error.message)
  }
  return resp.data
}

export interface ApiHealth {
  status: 'ok' | 'degraded' | 'down'
  version: string
  uptime_ms: number
  checks: HealthCheck[]
}

export interface HealthCheck {
  name: string
  status: 'ok' | 'warn' | 'fail'
  latency_ms?: number
  message?: string
}

export interface ApiVersionInfo {
  api_version: string
  build_version: string
  git_commit?: string
  build_date?: string
  features: string[]
}

export interface BatchRequest<T> {
  items: T[]
}

export interface BatchResponse<T> {
  succeeded: T[]
  failed: Array<{ item: T; error: string }>
}

export interface AckResponse {
  run_id: RunId
  acknowledged: boolean
  message?: string
}
