import type { AgentEvent } from './types'
import { mockEventsForRun } from './mock-data'
import type {
  IWSClient, WSStatus, WSEvent, WSMetrics, WSClientOptions,
} from './ws-client'

// ═══════════════════════════════════════════════════════════════════════
// MockWSClient — simulates WebSocket using mock data in mock mode
// ═══════════════════════════════════════════════════════════════════════

interface PlaybackTimer {
  timer: ReturnType<typeof setTimeout>
  event: AgentEvent
}

const EVENT_DELAY_BASE = 600
const EVENT_DELAY_JITTER = 800

export class MockWSClient implements IWSClient {
  private closed = false
  private _status: WSStatus = 'idle'
  private connectTimer: ReturnType<typeof setTimeout> | null = null
  private playbackTimers: PlaybackTimer[] = []
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null
  private subscriptions = new Set<string>()

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
    debug: boolean
  }

  constructor(opts: WSClientOptions) {
    this.opts = {
      debug: false,
      ...opts,
    }
  }

  connect(): void {
    this.closed = false
    this.setStatus('connecting')

    this.connectTimer = setTimeout(() => {
      if (this.closed) return
      this.metrics.connected_at = Date.now()
      this.setStatus('open')
      this.startPlayback()
      this.startHeartbeat()
    }, 300 + Math.random() * 200)
  }

  send(data: unknown): boolean {
    if (this._status !== 'open') return false

    const msg = typeof data === 'string' ? JSON.parse(data) : data

    if (msg?.type === 'ping') {
      this.handlePong()
      return true
    }

    if (msg?.type === 'subscribe') {
      this.subscriptions.add(msg.topic)
      this.emitEvent({ type: 'subscribed', topic: msg.topic })
      return true
    }

    if (msg?.type === 'unsubscribe') {
      this.subscriptions.delete(msg.topic)
      this.emitEvent({ type: 'unsubscribed', topic: msg.topic })
      return true
    }

    this.metrics.messages_sent += 1
    return true
  }

  close(): void {
    this.closed = true
    this.stopHeartbeat()
    this.clearTimers()
    this.metrics.connected_at = null
    this.setStatus('closed')
  }

  subscribe(topic: string): boolean {
    this.subscriptions.add(topic)
    return this.send({ type: 'subscribe', topic })
  }

  unsubscribe(topic: string): boolean {
    if (!this.subscriptions.has(topic)) return false
    this.subscriptions.delete(topic)
    return this.send({ type: 'unsubscribe', topic })
  }

  get status(): WSStatus {
    return this._status
  }

  getMetrics(): Readonly<WSMetrics> {
    return { ...this.metrics }
  }

  getActiveSubscriptions(): string[] {
    return [...this.subscriptions]
  }

  // ── Private ─────────────────────────────────────────────────────────

  private startPlayback(): void {
    const runId = this.extractRunId(this.opts.url)
    if (!runId) return

    const events = mockEventsForRun(runId)
    if (events.length === 0) return

    let cumulativeDelay = 500

    for (const event of events) {
      const delay = cumulativeDelay
      const timer = setTimeout(() => {
        if (this.closed || this._status !== 'open') return
        this.metrics.messages_received += 1
        this.metrics.last_message_at = Date.now()
        this.metrics.bytes_received += JSON.stringify(event).length
        this.opts.onMessage(event)
      }, delay)

      this.playbackTimers.push({ timer, event })
      cumulativeDelay += EVENT_DELAY_BASE + Math.random() * EVENT_DELAY_JITTER
    }
  }

  private extractRunId(url: string): string | null {
    const match = url.match(/\/ws\/runs\/([a-zA-Z0-9_-]+)/)
    return match?.[1] ?? null
  }

  private startHeartbeat(): void {
    this.heartbeatTimer = setInterval(() => {
      // No-op in mock mode; pings are responded to immediately in send()
    }, 30_000)
  }

  private stopHeartbeat(): void {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer)
      this.heartbeatTimer = null
    }
  }

  private handlePong(): void {
    // Instant pong response — no action needed
  }

  private clearTimers(): void {
    if (this.connectTimer) {
      clearTimeout(this.connectTimer)
      this.connectTimer = null
    }
    for (const { timer } of this.playbackTimers) {
      clearTimeout(timer)
    }
    this.playbackTimers = []
  }

  private setStatus(status: WSStatus): void {
    this._status = status
    this.opts.onStatus?.(status)
    this.emitEvent({ type: 'status', status })
  }

  private emitEvent(event: WSEvent): void {
    this.opts.onEvent?.(event)
    if (this.opts.debug) {
      console.debug('[MockWSClient]', event)
    }
  }
}
