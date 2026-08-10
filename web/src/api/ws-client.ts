import type { AgentEvent } from './types'

// ═══════════════════════════════════════════════════════════════════════
// Types
// ═══════════════════════════════════════════════════════════════════════

export type WSStatus = 'idle' | 'connecting' | 'open' | 'closed' | 'reconnecting' | 'error'

export interface HeartbeatConfig {
  interval: number
  timeout: number
}

export type WSEvent =
  | { type: 'status'; status: WSStatus }
  | { type: 'reconnect'; attempt: number }
  | { type: 'heartbeat_missed' }
  | { type: 'queue_overflow'; dropped: number }
  | { type: 'subscribed'; topic: string }
  | { type: 'unsubscribed'; topic: string }
  | { type: 'error'; error: Error }

export interface WSMetrics {
  connected_at: number | null
  messages_sent: number
  messages_received: number
  reconnect_count: number
  bytes_received: number
  last_message_at: number | null
}

export interface WSClientOptions {
  url: string
  onMessage: (event: AgentEvent) => void
  onStatus?: (status: WSStatus) => void
  onEvent?: (event: WSEvent) => void
  onReconnect?: (attempt: number) => void
  reconnectInterval?: number
  maxReconnectInterval?: number
  maxReconnectAttempts?: number
  heartbeat?: Partial<HeartbeatConfig>
  queueMessages?: boolean
  maxQueueSize?: number
  protocols?: string | string[]
  debug?: boolean
}

export interface IWSClient {
  connect(): void
  send(data: unknown): boolean
  close(code?: number, reason?: string): void
  subscribe(topic: string): boolean
  unsubscribe(topic: string): boolean
  readonly status: WSStatus
  getMetrics(): Readonly<WSMetrics>
  getActiveSubscriptions(): string[]
}

// ═══════════════════════════════════════════════════════════════════════
// WSClient — production WebSocket with heartbeat, queue, subscriptions
// ═══════════════════════════════════════════════════════════════════════

interface QueuedMessage {
  data: unknown
  ts: number
}

interface Subscription {
  topic: string
  active: boolean
}

const PING_MSG = JSON.stringify({ type: 'ping' })

export class WSClient implements IWSClient {
  private ws: WebSocket | null = null
  private reconnectAttempts = 0
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private closed = false

  private heartbeatTimer: ReturnType<typeof setInterval> | null = null
  private pongTimer: ReturnType<typeof setTimeout> | null = null
  private heartbeatWaiting = false

  private messageQueue: QueuedMessage[] = []
  private subscriptions = new Map<string, Subscription>()

  private metrics: WSMetrics = {
    connected_at: null,
    messages_sent: 0,
    messages_received: 0,
    reconnect_count: 0,
    bytes_received: 0,
    last_message_at: null,
  }

  private opts: {
    url: string
    onMessage: (event: AgentEvent) => void
    onStatus?: (status: WSStatus) => void
    onEvent?: (event: WSEvent) => void
    onReconnect?: (attempt: number) => void
    reconnectInterval: number
    maxReconnectInterval: number
    maxReconnectAttempts: number
    heartbeat: HeartbeatConfig
    queueMessages: boolean
    maxQueueSize: number
    protocols?: string | string[]
    debug: boolean
  }

  constructor(opts: WSClientOptions) {
    this.opts = {
      reconnectInterval: 1000,
      maxReconnectInterval: 30_000,
      maxReconnectAttempts: Infinity,
      queueMessages: false,
      maxQueueSize: 100,
      debug: false,
      protocols: undefined,
      ...opts,
      heartbeat: { interval: 30_000, timeout: 10_000, ...opts.heartbeat },
    }
  }

  // ── Connection ──────────────────────────────────────────────────────

  connect(): void {
    this.closed = false
    this.setStatus('connecting')

    try {
      const url = this.resolveUrl(this.opts.url)
      this.ws = this.opts.protocols
        ? new WebSocket(url, this.opts.protocols)
        : new WebSocket(url)
    } catch {
      this.scheduleReconnect()
      return
    }

    this.ws.onopen = () => {
      this.reconnectAttempts = 0
      this.metrics.connected_at = Date.now()
      this.setStatus('open')
      this.flushQueue()
      this.resubscribeAll()
      this.startHeartbeat()
    }

    this.ws.onmessage = (e: MessageEvent) => {
      this.metrics.messages_received += 1
      this.metrics.last_message_at = Date.now()

      const raw = typeof e.data === 'string' ? e.data : ''
      this.metrics.bytes_received += raw.length

      try {
        const data = JSON.parse(raw)

        if (data?.type === 'pong') {
          this.handlePong()
          return
        }

        if (data?.type === 'subscribed') {
          const sub = this.subscriptions.get(data.topic as string)
          if (sub) sub.active = true
          this.emitEvent({ type: 'subscribed', topic: data.topic })
          return
        }

        if (data?.type === 'unsubscribed') {
          this.subscriptions.delete(data.topic as string)
          this.emitEvent({ type: 'unsubscribed', topic: data.topic })
          return
        }

        this.opts.onMessage(data as AgentEvent)
      } catch {
        // ignore malformed payloads
      }
    }

    this.ws.onerror = () => {
      this.emitEvent({ type: 'error', error: new Error('WebSocket error') })
      this.setStatus('error')
    }

    this.ws.onclose = () => {
      this.stopHeartbeat()
      this.metrics.connected_at = null
      if (this.closed) return
      this.setStatus('closed')
      this.scheduleReconnect()
    }
  }

  send(data: unknown): boolean {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(typeof data === 'string' ? data : JSON.stringify(data))
      this.metrics.messages_sent += 1
      return true
    }

    if (this.opts.queueMessages) {
      if (this.messageQueue.length >= this.opts.maxQueueSize) {
        const dropped = this.messageQueue.length - this.opts.maxQueueSize + 1
        this.messageQueue.splice(0, dropped)
        this.emitEvent({ type: 'queue_overflow', dropped })
      }
      this.messageQueue.push({ data, ts: Date.now() })
    }

    return false
  }

  close(code?: number, reason?: string): void {
    this.closed = true
    this.stopHeartbeat()

    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }

    if (this.ws) {
      this.ws.onclose = null
      this.ws.onerror = null
      this.ws.onmessage = null
      this.ws.onopen = null
      try {
        this.ws.close(code, reason)
      } catch {
        // ignore
      }
      this.ws = null
    }

    this.messageQueue = []
    this.setStatus('closed')
  }

  // ── Subscriptions ───────────────────────────────────────────────────

  subscribe(topic: string): boolean {
    this.subscriptions.set(topic, { topic, active: false })
    return this.send({ type: 'subscribe', topic })
  }

  unsubscribe(topic: string): boolean {
    const sub = this.subscriptions.get(topic)
    if (!sub) return false

    const sent = this.send({ type: 'unsubscribe', topic })
    if (sent) {
      this.subscriptions.delete(topic)
    } else {
      sub.active = false
    }
    return sent
  }

  // ── Heartbeat ───────────────────────────────────────────────────────

  private startHeartbeat(): void {
    this.stopHeartbeat()
    if (this.opts.heartbeat.interval <= 0) return

    this.heartbeatTimer = setInterval(() => {
      if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return

      this.heartbeatWaiting = true
      this.ws.send(PING_MSG)

      this.pongTimer = setTimeout(() => {
        if (this.heartbeatWaiting) {
          this.emitEvent({ type: 'heartbeat_missed' })
          this.forceReconnect()
        }
      }, this.opts.heartbeat.timeout)
    }, this.opts.heartbeat.interval)
  }

  private stopHeartbeat(): void {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer)
      this.heartbeatTimer = null
    }
    if (this.pongTimer) {
      clearTimeout(this.pongTimer)
      this.pongTimer = null
    }
    this.heartbeatWaiting = false
  }

  private handlePong(): void {
    this.heartbeatWaiting = false
    if (this.pongTimer) {
      clearTimeout(this.pongTimer)
      this.pongTimer = null
    }
  }

  // ── Message Queue ───────────────────────────────────────────────────

  private flushQueue(): void {
    if (this.messageQueue.length === 0) return
    const queued = [...this.messageQueue]
    this.messageQueue = []
    for (const msg of queued) {
      this.send(msg.data)
    }
  }

  get queuedCount(): number {
    return this.messageQueue.length
  }

  // ── Reconnection ────────────────────────────────────────────────────

  private forceReconnect(): void {
    if (this.ws) {
      try {
        this.ws.close()
      } catch {
        // ignore
      }
    }
    // onclose handler will trigger scheduleReconnect
  }

  private resubscribeAll(): void {
    for (const [topic, sub] of this.subscriptions) {
      sub.active = false
      this.send({ type: 'subscribe', topic })
    }
  }

  private scheduleReconnect(): void {
    if (this.closed) return
    if (this.reconnectAttempts >= this.opts.maxReconnectAttempts) {
      this.emitEvent({
        type: 'error',
        error: new Error('Max reconnection attempts reached'),
      })
      return
    }

    this.reconnectAttempts += 1
    this.metrics.reconnect_count += 1

    const base = this.opts.reconnectInterval
    const max = this.opts.maxReconnectInterval
    const jitter = Math.random() * 500
    const delay = Math.min(base * 2 ** (this.reconnectAttempts - 1) + jitter, max)

    this.setStatus('reconnecting')
    this.emitEvent({ type: 'reconnect', attempt: this.reconnectAttempts })
    this.opts.onReconnect?.(this.reconnectAttempts)

    this.reconnectTimer = setTimeout(() => {
      if (!this.closed) this.connect()
    }, delay)
  }

  // ── Accessors ───────────────────────────────────────────────────────

  get status(): WSStatus {
    if (this.closed) return 'closed'
    if (!this.ws) return 'idle'
    switch (this.ws.readyState) {
      case WebSocket.CONNECTING: return 'connecting'
      case WebSocket.OPEN: return 'open'
      case WebSocket.CLOSING: return 'closed'
      case WebSocket.CLOSED: return 'closed'
      default: return 'idle'
    }
  }

  getMetrics(): Readonly<WSMetrics> {
    return { ...this.metrics }
  }

  getActiveSubscriptions(): string[] {
    return [...this.subscriptions.values()]
      .filter((s) => s.active)
      .map((s) => s.topic)
  }

  // ── Private Helpers ─────────────────────────────────────────────────

  private resolveUrl(path: string): string {
    if (/^wss?:\/\//.test(path)) return path
    const proto = globalThis.location?.protocol === 'https:' ? 'wss:' : 'ws:'
    const host = globalThis.location?.host ?? 'localhost'
    return `${proto}//${host}${path}`
  }

  private setStatus(status: WSStatus): void {
    this.opts.onStatus?.(status)
    this.emitEvent({ type: 'status', status })
  }

  private emitEvent(event: WSEvent): void {
    this.opts.onEvent?.(event)
    if (this.opts.debug) {
      console.debug('[WSClient]', event)
    }
  }
}
