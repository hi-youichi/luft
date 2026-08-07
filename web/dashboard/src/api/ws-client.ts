// ============================================================================
// LuftWsClient — Protocol-agnostic WebSocket transport with auto-reconnect.
//
// Provides connection state management, exponential-backoff reconnection,
// message queueing while disconnected, and a clean event subscription API.
// Works in any browser that supports the native WebSocket API.
// ============================================================================

import type { ConnectionState } from './types.js';

// ── Configuration ─────────────────────────────────────────────────────────

export interface ReconnectOptions {
  initialDelayMs: number;
  maxDelayMs: number;
  multiplier: number;
  maxRetries: number;
  jitterMs: number;
}

export interface WsClientConfig {
  url: string;
  protocols?: string | string[];
  reconnect?: boolean;
  reconnectOptions?: Partial<ReconnectOptions>;
  connectTimeoutMs?: number;
}

const DEFAULT_RECONNECT: ReconnectOptions = {
  initialDelayMs: 1_000,
  maxDelayMs: 30_000,
  multiplier: 2,
  maxRetries: Infinity,
  jitterMs: 500,
};

const DEFAULT_CONNECT_TIMEOUT = 10_000;

type MessageHandler = (data: string) => void;
type StateHandler = (state: ConnectionState, prev: ConnectionState) => void;
type ErrorHandler = (error: Error) => void;

// ── LuftWsClient ──────────────────────────────────────────────────────────

export class LuftWsClient {
  private ws: WebSocket | null = null;
  private _state: ConnectionState = 'disconnected';
  private explicitlyClosed = false;
  private reconnectAttempts = 0;

  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private connectTimer: ReturnType<typeof setTimeout> | null = null;
  private lastMessageTime = 0;

  private readonly config: {
    url: string;
    protocols: string | string[];
    reconnect: boolean;
    reconnectOptions: ReconnectOptions;
    connectTimeoutMs: number;
  };

  private readonly messageHandlers = new Set<MessageHandler>();
  private readonly stateHandlers = new Set<StateHandler>();
  private readonly errorHandlers = new Set<ErrorHandler>();
  private readonly queue: string[] = [];

  // Pending connect promise — resolves on first open, rejects on error/timeout
  private connectPromise: {
    resolve: () => void;
    reject: (e: Error) => void;
  } | null = null;

  constructor(config: WsClientConfig) {
    this.config = {
      url: config.url,
      protocols: config.protocols ?? [],
      reconnect: config.reconnect ?? true,
      reconnectOptions: { ...DEFAULT_RECONNECT, ...config.reconnectOptions },
      connectTimeoutMs: config.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT,
    };
  }

  get state(): ConnectionState {
    return this._state;
  }

  get isConnected(): boolean {
    return this._state === 'connected';
  }

  get url(): string {
    return this.config.url;
  }

  get lastMessageAt(): number {
    return this.lastMessageTime;
  }

  // ── Lifecycle ──────────────────────────────────────────────────────────

  connect(): Promise<void> {
    if (this._state === 'connected' || this._state === 'connecting') {
      return Promise.resolve();
    }
    this.explicitlyClosed = false;
    this.setState('connecting');
    return this.open();
  }

  disconnect(): void {
    this.explicitlyClosed = true;
    this.clearTimers();
    this.rejectConnect(new Error('Connection closed by client'));
    if (this.ws) {
      try {
        this.ws.onclose = null;
        this.ws.onerror = null;
        this.ws.onopen = null;
        this.ws.onmessage = null;
        this.ws.close(1000, 'client disconnect');
      } catch {
        // ignore
      }
      this.ws = null;
    }
    this.setState('disconnected');
  }

  // ── Sending ────────────────────────────────────────────────────────────

  send(data: string): boolean {
    if (!this.isConnected || !this.ws) {
      this.queue.push(data);
      return false;
    }
    this.ws.send(data);
    return true;
  }

  sendJSON(data: unknown): boolean {
    return this.send(JSON.stringify(data));
  }

  flushQueue(): void {
    if (!this.ws) return;
    while (this.queue.length > 0) {
      this.ws.send(this.queue.shift()!);
    }
  }

  // ── Event subscriptions (each returns an unsubscribe fn) ───────────────

  onMessage(handler: MessageHandler): () => void {
    this.messageHandlers.add(handler);
    return () => { this.messageHandlers.delete(handler); };
  }

  onStateChange(handler: StateHandler): () => void {
    this.stateHandlers.add(handler);
    return () => { this.stateHandlers.delete(handler); };
  }

  onError(handler: ErrorHandler): () => void {
    this.errorHandlers.add(handler);
    return () => { this.errorHandlers.delete(handler); };
  }

  // ── Cleanup ────────────────────────────────────────────────────────────

  dispose(): void {
    this.disconnect();
    this.messageHandlers.clear();
    this.stateHandlers.clear();
    this.errorHandlers.clear();
    this.queue.length = 0;
  }

  // ── Private: connection ────────────────────────────────────────────────

  private open(): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      this.connectPromise = { resolve, reject };

      let ws: WebSocket;
      try {
        ws = this.config.protocols.length > 0
          ? new WebSocket(this.config.url, this.config.protocols)
          : new WebSocket(this.config.url);
      } catch (err) {
        this.setState('error');
        const e = err instanceof Error ? err : new Error(String(err));
        this.emitError(e);
        reject(e);
        return;
      }

      this.ws = ws;

      // Connect timeout
      this.connectTimer = setTimeout(() => {
        if (this._state !== 'connected') {
          this.cleanupSocket(ws);
          const e = new Error(`WebSocket connect timeout after ${this.config.connectTimeoutMs}ms`);
          this.handleOpenError(e);
        }
      }, this.config.connectTimeoutMs);

      ws.onopen = () => {
        this.clearTimer('connect');
        this.reconnectAttempts = 0;
        this.lastMessageTime = Date.now();
        this.setState('connected');
        this.flushQueue();
        this.resolveConnect();
      };

      ws.onmessage = (ev: MessageEvent) => {
        this.lastMessageTime = Date.now();
        const data = typeof ev.data === 'string' ? ev.data : '';
        if (data) this.emitMessage(data);
      };

      ws.onerror = () => {
        // The browser fires onclose after onerror; we let onclose drive reconnect.
        // But during connecting, reject immediately.
        if (this._state === 'connecting' || this._state === 'reconnecting') {
          this.cleanupSocket(ws);
          this.handleOpenError(new Error('WebSocket connection failed'));
        }
      };

      ws.onclose = (ev: CloseEvent) => {
        this.cleanupSocket(ws);
        if (this._state === 'connected') {
          // Unexpected close after being connected
          this.emitError(new Error(`WebSocket closed: ${ev.code} ${ev.reason}`));
        }
        if (this.explicitlyClosed) {
          this.setState('disconnected');
          this.rejectConnect(new Error('Connection closed'));
          return;
        }
        this.rejectConnect(new Error('Connection closed'));
        if (this.config.reconnect) {
          this.scheduleReconnect();
        } else {
          this.setState('disconnected');
        }
      };
    });
  }

  private handleOpenError(e: Error): void {
    this.emitError(e);
    this.rejectConnect(e);
    if (this.config.reconnect && !this.explicitlyClosed) {
      this.scheduleReconnect();
    } else {
      this.setState('disconnected');
    }
  }

  private scheduleReconnect(): void {
    const opts = this.config.reconnectOptions;
    if (this.reconnectAttempts >= opts.maxRetries) {
      this.setState('error');
      this.emitError(new Error(`Max reconnect attempts (${opts.maxRetries}) exceeded`));
      return;
    }

    this.reconnectAttempts++;
    const base = Math.min(
      opts.initialDelayMs * Math.pow(opts.multiplier, this.reconnectAttempts - 1),
      opts.maxDelayMs,
    );
    const jitter = opts.jitterMs > 0 ? Math.random() * opts.jitterMs : 0;
    const delay = Math.round(base + jitter);

    this.setState('reconnecting');
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (this.explicitlyClosed) return;
      this.open().catch(() => {
        // open() already handles error/reject via its own callbacks
      });
    }, delay);
  }

  // ── Private: state & events ────────────────────────────────────────────

  private setState(next: ConnectionState): void {
    if (this._state === next) return;
    const prev = this._state;
    this._state = next;
    for (const h of this.stateHandlers) {
      try { h(next, prev); } catch { /* handler errors are swallowed */ }
    }
  }

  private emitMessage(data: string): void {
    for (const h of this.messageHandlers) {
      try { h(data); } catch { /* handler errors are swallowed */ }
    }
  }

  private emitError(e: Error): void {
    for (const h of this.errorHandlers) {
      try { h(e); } catch { /* handler errors are swallowed */ }
    }
  }

  // ── Private: timers & socket ───────────────────────────────────────────

  private clearTimer(which: 'connect' | 'reconnect'): void {
    if (which === 'connect' && this.connectTimer) {
      clearTimeout(this.connectTimer);
      this.connectTimer = null;
    }
    if (which === 'reconnect' && this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  private clearTimers(): void {
    this.clearTimer('connect');
    this.clearTimer('reconnect');
  }

  private cleanupSocket(ws: WebSocket): void {
    if (this.ws === ws) {
      this.ws = null;
    }
    ws.onopen = null;
    ws.onmessage = null;
    ws.onerror = null;
    ws.onclose = null;
  }

  private resolveConnect(): void {
    if (this.connectPromise) {
      this.connectPromise.resolve();
      this.connectPromise = null;
    }
  }

  private rejectConnect(e: Error): void {
    if (this.connectPromise) {
      this.connectPromise.reject(e);
      this.connectPromise = null;
    }
  }
}
