import { httpClient, type Interceptor, type RequestOptions } from './http-client'
import { ApiError } from './errors'

export interface AuthTokenProvider {
  get(): string | null
}

let tokenProvider: AuthTokenProvider | null = null

const AUTH_HEADER = 'Authorization'

const authInterceptor: Interceptor<RequestOptions> = {
  id: 'auth',
  fn(opts) {
    const token = tokenProvider?.get()
    if (token) {
      opts.headers = { ...opts.headers, [AUTH_HEADER]: `Bearer ${token}` }
    }
    return opts
  },
}

const loggingInterceptor: Interceptor<RequestOptions> = {
  id: 'logging',
  fn(opts) {
    const method = opts.method ?? 'GET'
    const tag = import.meta.env.DEV ? '[HTTP]' : ''
    if (tag) {
      console.debug(tag, method, opts.params ? JSON.stringify(opts.params) : '')
    }
    return opts
  },
}

export function setupInterceptors(): void {
  httpClient.addRequestInterceptor(authInterceptor)
  httpClient.addRequestInterceptor(loggingInterceptor)
}

export function setAuthTokenProvider(provider: AuthTokenProvider): void {
  tokenProvider = provider
}

export function setAuthToken(token: string | null): void {
  tokenProvider = token
    ? { get: () => token }
    : null
}

export function setupApi(options?: {
  authToken?: string | null
  tokenProvider?: AuthTokenProvider
}): void {
  if (options?.tokenProvider) {
    setAuthTokenProvider(options.tokenProvider)
  } else if (options?.authToken !== undefined) {
    setAuthToken(options.authToken)
  }
  setupInterceptors()
}

export function isApiError(err: unknown): err is ApiError {
  return err instanceof ApiError
}
