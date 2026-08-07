import { apiConfig } from './config'
import { WSClient, type WSClientOptions, type IWSClient } from './ws-client'
import { MockWSClient } from './ws-mock'

// ═══════════════════════════════════════════════════════════════════════
// URL Helpers
// ═══════════════════════════════════════════════════════════════════════

export function httpToWs(url: string): string {
  return url.replace(/^http/, 'ws')
}

export function buildWSUrl(path: string): string {
  const proto = globalThis.location?.protocol === 'https:' ? 'wss:' : 'ws:'
  const host = globalThis.location?.host ?? 'localhost'
  const base = apiConfig.baseURL.replace(/\/$/, '')

  if (/^wss?:\/\//.test(path)) return path
  if (path.startsWith('/')) return `${proto}//${host}${path}`

  return `${proto}//${host}${base}/${path}`
}

export function buildRunWSUrl(runId: string): string {
  return buildWSUrl(`/ws/runs/${runId}`)
}

// ═══════════════════════════════════════════════════════════════════════
// Factory
// ═══════════════════════════════════════════════════════════════════════

export function createWSClient(opts: WSClientOptions): IWSClient {
  if (apiConfig.mode === 'mock') {
    return new MockWSClient(opts)
  }
  return new WSClient(opts)
}

export function createRunWSClient(
  runId: string,
  handlers: Pick<WSClientOptions, 'onMessage' | 'onStatus' | 'onEvent'>,
  extra?: Partial<Omit<WSClientOptions, 'url' | 'onMessage'>>,
): IWSClient {
  return createWSClient({
    url: buildRunWSUrl(runId),
    ...handlers,
    ...extra,
  })
}

export function isMockMode(): boolean {
  return apiConfig.mode === 'mock'
}

export type { IWSClient, WSClientOptions }
