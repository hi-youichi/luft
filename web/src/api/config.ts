export type ApiMode = 'mock' | 'live'

export interface ApiConfig {
  baseURL: string
  timeout: number
  retries: number
  retryDelay: number
  retryMaxDelay: number
  mode: ApiMode
  defaultHeaders: Record<string, string>
}

const envBaseURL = (import.meta.env.VITE_API_BASE_URL as string | undefined) ?? ''

export const apiConfig: ApiConfig = {
  baseURL: envBaseURL || '/api',
  timeout: 30_000,
  retries: 3,
  retryDelay: 1_000,
  retryMaxDelay: 10_000,
  mode: (import.meta.env.VITE_API_MODE as ApiMode | undefined) ?? 'mock',
  defaultHeaders: {
    'Content-Type': 'application/json',
  },
}

export function updateApiConfig(partial: Partial<ApiConfig>): void {
  Object.assign(apiConfig, partial)
}
