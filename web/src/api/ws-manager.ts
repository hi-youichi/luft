import type { AgentEvent } from './types'
import { createWSClient } from './ws-factory'
import type { IWSClient, WSStatus, WSEvent, WSClientOptions } from './ws-client'

// ═══════════════════════════════════════════════════════════════════════
// Types
// ═══════════════════════════════════════════════════════════════════════

export type MessageHandler = (event: AgentEvent) => void
export type StatusHandler = (status: WSStatus) => void

export interface WSManagerSubscription {
  unsubscribe(): void
}

interface ManagedConnection {
  client: IWSClient
  url: string
  refCount: number
  messageHandlers: Set<MessageHandler>
  statusHandlers: Set<StatusHandler>
  cleanupTimer: ReturnType<typeof setTimeout> | null
  createdAt: number
}

export interface WSManagerMetrics {
  total_connections: number
  active_connections: number
  total_messages_received: number
  total_reconnects: number
}

export interface WSManagerOptions {
  idleTimeout?: number
  debug?: boolean
}

// ═══════════════════════════════════════════════════════════════════════
// WSManager — multi-subscriber WebSocket connection pool
// ═══════════════════════════════════════════════════════════════════════

const DEFAULT_IDLE_TIMEOUT = 5_000

export class WSManager {
  private connections = new Map<string, ManagedConnection>()
  private idleTimeout: number
  private debug: boolean

  private metrics: WSManagerMetrics = {
    total_connections: 0,
    active_connections: 0,
    total_messages_received: 0,
    total_reconnects: 0,
  }

  constructor(options: WSManagerOptions = {}) {
    this.idleTimeout = options.idleTimeout ?? DEFAULT_IDLE_TIMEOUT
    this.debug = options.debug ?? false
  }

  // ── Public API ──────────────────────────────────────────────────────

  subscribe(
    url: string,
    handlers: {
      onMessage: MessageHandler
      onStatus?: StatusHandler
    },
    extra?: Partial<Omit<WSClientOptions, 'url' | 'onMessage' | 'onStatus'>>,
  ): WSManagerSubscription {
    let conn = this.connections.get(url)

    if (!conn) {
      conn = this.createConnection(url, extra)
      this.connections.set(url, conn)
    }

    if (conn.cleanupTimer) {
      clearTimeout(conn.cleanupTimer)
      conn.cleanupTimer = null
    }

    conn.refCount += 1
    conn.messageHandlers.add(handlers.onMessage)
    if (handlers.onStatus) {
      conn.statusHandlers.add(handlers.onStatus)
      handlers.onStatus(conn.client.status)
    }

    this.log(`Subscribed to ${url}, refs: ${conn.refCount}`)

    return {
      unsubscribe: () => {
        this.unsubscribe(url, handlers.onMessage, handlers.onStatus)
      },
    }
  }

  unsubscribe(
    url: string,
    messageHandler: MessageHandler,
    statusHandler?: StatusHandler,
  ): void {
    const conn = this.connections.get(url)
    if (!conn) return

    conn.messageHandlers.delete(messageHandler)
    if (statusHandler) conn.statusHandlers.delete(statusHandler)
    conn.refCount -= 1

    this.log(`Unsubscribed from ${url}, refs: ${conn.refCount}`)

    if (conn.refCount <= 0) {
      conn.cleanupTimer = setTimeout(() => {
        this.closeConnection(url)
      }, this.idleTimeout)
    }
  }

  getConnectionStatus(url: string): WSStatus | null {
    const conn = this.connections.get(url)
    return conn ? conn.client.status : null
  }

  getActiveUrls(): string[] {
    return [...this.connections.values()]
      .filter((c) => c.refCount > 0)
      .map((c) => c.url)
  }

  getMetrics(): Readonly<WSManagerMetrics> {
    const active = [...this.connections.values()].filter((c) => c.refCount > 0)
    let totalMessages = 0
    let totalReconnects = 0

    for (const conn of this.connections.values()) {
      const m = conn.client.getMetrics()
      totalMessages += m.messages_received
      totalReconnects += m.reconnect_count
    }

    return {
      total_connections: this.metrics.total_connections,
      active_connections: active.length,
      total_messages_received: totalMessages,
      total_reconnects: totalReconnects,
    }
  }

  closeAll(): void {
    for (const url of [...this.connections.keys()]) {
      this.closeConnection(url)
    }
  }

  // ── Private ─────────────────────────────────────────────────────────

  private createConnection(
    url: string,
    extra?: Partial<Omit<WSClientOptions, 'url' | 'onMessage' | 'onStatus'>>,
  ): ManagedConnection {
    const conn: ManagedConnection = {
      client: null as unknown as IWSClient,
      url,
      refCount: 0,
      messageHandlers: new Set(),
      statusHandlers: new Set(),
      cleanupTimer: null,
      createdAt: Date.now(),
    }

    const client = createWSClient({
      url,
      onMessage: (event: AgentEvent) => {
        this.metrics.total_messages_received += 1
        for (const handler of conn.messageHandlers) {
          handler(event)
        }
      },
      onStatus: (status: WSStatus) => {
        for (const handler of conn.statusHandlers) {
          handler(status)
        }
      },
      onEvent: (event: WSEvent) => {
        if (event.type === 'reconnect') {
          this.metrics.total_reconnects += 1
        }
        this.log(`Event on ${url}:`, event.type)
      },
      ...extra,
    })

    conn.client = client
    this.metrics.total_connections += 1

    client.connect()

    return conn
  }

  private closeConnection(url: string): void {
    const conn = this.connections.get(url)
    if (!conn) return

    if (conn.cleanupTimer) {
      clearTimeout(conn.cleanupTimer)
      conn.cleanupTimer = null
    }

    conn.client.close()
    this.connections.delete(url)

    this.log(`Closed connection: ${url}`)
  }

  private log(...args: unknown[]): void {
    if (this.debug) {
      console.debug('[WSManager]', ...args)
    }
  }
}

// ═══════════════════════════════════════════════════════════════════════
// Singleton
// ═══════════════════════════════════════════════════════════════════════

let _instance: WSManager | null = null

export function getWSManager(): WSManager {
  if (!_instance) {
    _instance = new WSManager()
  }
  return _instance
}

export function resetWSManager(): void {
  _instance?.closeAll()
  _instance = null
}
