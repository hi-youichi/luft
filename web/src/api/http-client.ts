import { apiConfig } from './config'
import { ApiError, NetworkError, TimeoutError } from './errors'

export type HttpMethod = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'

export interface RequestOptions {
  method?: HttpMethod
  body?: unknown
  headers?: Record<string, string>
  timeout?: number
  retries?: number
  signal?: AbortSignal
  params?: Record<string, string | number | boolean | undefined>
}

export interface Interceptor<T> {
  id: string
  fn: (value: T) => T | Promise<T>
}

const RETRYABLE_STATUS = new Set([408, 429, 500, 502, 503, 504])

function buildURL(base: string, path: string, params?: RequestOptions['params']): string {
  const url = new URL(`${base}${path}`, globalThis.location?.origin ?? 'http://localhost')
  if (params) {
    for (const [key, value] of Object.entries(params)) {
      if (value !== undefined && value !== null) {
        url.searchParams.set(key, String(value))
      }
    }
  }
  return url.pathname + url.search
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(resolve, ms)
    signal?.addEventListener('abort', () => {
      clearTimeout(timer)
      reject(new NetworkError('Request aborted'))
    }, { once: true })
  })
}

function isRetryable(error: unknown, attempt: number, maxRetries: number): boolean {
  if (attempt >= maxRetries) return false
  if (error instanceof ApiError) return RETRYABLE_STATUS.has(error.status)
  if (error instanceof NetworkError || error instanceof TimeoutError) return true
  return false
}

class HttpClient {
  private requestInterceptors: Interceptor<RequestOptions>[] = []
  private responseInterceptors: Interceptor<Response>[] = []

  addRequestInterceptor(interceptor: Interceptor<RequestOptions>): void {
    this.requestInterceptors.push(interceptor)
  }

  removeRequestInterceptor(id: string): void {
    this.requestInterceptors = this.requestInterceptors.filter(i => i.id !== id)
  }

  addResponseInterceptor(interceptor: Interceptor<Response>): void {
    this.responseInterceptors.push(interceptor)
  }

  removeResponseInterceptor(id: string): void {
    this.responseInterceptors = this.responseInterceptors.filter(i => i.id !== id)
  }

  async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    let opts = { ...options }
    for (const interceptor of this.requestInterceptors) {
      opts = await interceptor.fn(opts)
    }

    const {
      method = 'GET',
      body,
      headers = {},
      timeout = apiConfig.timeout,
      retries = apiConfig.retries,
      signal,
      params,
    } = opts

    const mergedHeaders: Record<string, string> = {
      ...apiConfig.defaultHeaders,
      ...headers,
    }

    const url = buildURL(apiConfig.baseURL, path, params)
    let lastError: unknown

    for (let attempt = 0; attempt <= retries; attempt++) {
      if (attempt > 0) {
        const backoff = Math.min(
          apiConfig.retryDelay * 2 ** (attempt - 1),
          apiConfig.retryMaxDelay,
        )
        await sleep(backoff + Math.random() * 500, signal)
      }

      const controller = new AbortController()
      const timeoutId = setTimeout(() => controller.abort(), timeout)

      if (signal) {
        signal.addEventListener('abort', () => controller.abort(), { once: true })
      }

      try {
        const init: RequestInit = {
          method,
          headers: mergedHeaders,
          signal: controller.signal,
        }
        if (body !== undefined && method !== 'GET') {
          init.body = JSON.stringify(body)
        }

        const response = await fetch(url, init)
        clearTimeout(timeoutId)

        let resp = response
        for (const interceptor of this.responseInterceptors) {
          resp = await interceptor.fn(resp)
        }

        if (!resp.ok) {
          let errorBody: unknown
          try {
            errorBody = await resp.json()
          } catch {
            try {
              errorBody = await resp.text()
            } catch {
              errorBody = undefined
            }
          }
          const message =
            (errorBody as Record<string, string>)?.message
            ?? (errorBody as Record<string, string>)?.error
            ?? resp.statusText
          const error = new ApiError(message, resp.status, errorBody)
          if (!isRetryable(error, attempt, retries)) throw error
          lastError = error
          continue
        }

        const contentType = resp.headers.get('content-type') ?? ''
        if (contentType.includes('application/json')) {
          return await resp.json() as T
        }
        if (contentType.includes('text/')) {
          return await resp.text() as T
        }
        return undefined as T
      } catch (err) {
        clearTimeout(timeoutId)

        if (err instanceof ApiError) {
          if (!isRetryable(err, attempt, retries)) throw err
          lastError = err
          continue
        }

        if (err instanceof DOMException && err.name === 'AbortError') {
          if (signal?.aborted) throw new NetworkError('Request aborted')
          const error = new TimeoutError(timeout)
          if (!isRetryable(error, attempt, retries)) throw error
          lastError = error
          continue
        }

        const error = new NetworkError(err instanceof Error ? err.message : String(err))
        if (!isRetryable(error, attempt, retries)) throw error
        lastError = error
      }
    }

    throw lastError
  }

  get<T>(path: string, options?: Omit<RequestOptions, 'method' | 'body'>): Promise<T> {
    return this.request<T>(path, { ...options, method: 'GET' })
  }

  post<T>(path: string, body?: unknown, options?: Omit<RequestOptions, 'method' | 'body'>): Promise<T> {
    return this.request<T>(path, { ...options, method: 'POST', body })
  }

  put<T>(path: string, body?: unknown, options?: Omit<RequestOptions, 'method' | 'body'>): Promise<T> {
    return this.request<T>(path, { ...options, method: 'PUT', body })
  }

  patch<T>(path: string, body?: unknown, options?: Omit<RequestOptions, 'method' | 'body'>): Promise<T> {
    return this.request<T>(path, { ...options, method: 'PATCH', body })
  }

  delete<T>(path: string, options?: Omit<RequestOptions, 'method' | 'body'>): Promise<T> {
    return this.request<T>(path, { ...options, method: 'DELETE' })
  }
}

export const httpClient = new HttpClient()
