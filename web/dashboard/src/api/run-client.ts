// ============================================================================
// RunClient — Real-time run session streaming client for the /run endpoint.
//
// Each RunSession represents one WebSocket connection.  The client sends a
// "start" message, then receives a stream of AgentEvent objects until the
// server sends "complete".  The session can be cancelled at any time.
// ============================================================================

import { LuftWsClient } from './ws-client.js';
import type {
  WsClientConfig,
} from './ws-client.js';
import type {
  AgentEvent,
  RunClientMessage,
  RunServerMessage,
} from './types.js';
import { isRunServerMessage } from './types.js';

// ── Types ─────────────────────────────────────────────────────────────────

export interface StartOptions {
  script?: string;
  resumeFromId?: string;
  onEvent?: (event: AgentEvent) => void;
}

export interface RunSession {
  runId: string;
  cancel(): void;
  onEvent(handler: (event: AgentEvent) => void): () => void;
  onComplete(handler: (result: RunCompleteResult) => void): () => void;
  onError(handler: (error: Error) => void): () => void;
  wait(): Promise<RunCompleteResult>;
}

export interface RunCompleteResult {
  runId: string;
  status: string;
}

// ── RunClient ─────────────────────────────────────────────────────────────

export class RunClient {
  private readonly ws: LuftWsClient;
  private activeSession: RunSessionImpl | null = null;
  private unsubMessage: (() => void) | null = null;

  constructor(ws: LuftWsClient) {
    this.ws = ws;
    this.unsubMessage = this.ws.onMessage((data) => this.handleMessage(data));
  }

  get isActive(): boolean {
    return this.activeSession !== null;
  }

  async start(options: StartOptions): Promise<RunSession> {
    if (this.activeSession) {
      throw new LuftWsErrorWrapper('A run session is already active');
    }
    if (!options.script && !options.resumeFromId) {
      throw new LuftWsErrorWrapper('Either script or resumeFromId must be provided');
    }

    await this.ws.connect();

    const session = new RunSessionImpl(this.ws, options.onEvent);
    this.activeSession = session;

    const msg: RunClientMessage = options.resumeFromId
      ? { type: 'start', resume_from_id: options.resumeFromId }
      : { type: 'start', script: options.script };

    this.ws.sendJSON(msg);

    return new Promise<RunSession>((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (!session.runId) {
          this.activeSession = null;
          session.dispose();
          reject(new LuftWsErrorWrapper('Timeout waiting for run to start'));
        }
      }, 15_000);

      session.onceStarted((err) => {
        clearTimeout(timeout);
        if (err) {
          this.activeSession = null;
          session.dispose();
          reject(err);
        } else {
          resolve(session);
        }
      });
    });
  }

  dispose(): void {
    this.activeSession?.dispose();
    this.activeSession = null;
    this.unsubMessage?.();
    this.unsubMessage = null;
    this.ws.dispose();
  }

  // ── Private ────────────────────────────────────────────────────────────

  private handleMessage(data: string): void {
    if (!this.activeSession) return;

    let msg: unknown;
    try {
      msg = JSON.parse(data);
    } catch {
      return;
    }

    if (!isRunServerMessage(msg)) return;

    this.activeSession.handleMessage(msg);
    if (this.activeSession.done) {
      this.activeSession = null;
    }
  }
}

// ── RunSessionImpl ────────────────────────────────────────────────────────

class RunSessionImpl implements RunSession {
  private readonly ws: LuftWsClient;
  private _runId = '';
  private _done = false;

  private readonly eventHandlers = new Set<(e: AgentEvent) => void>();
  private readonly completeHandlers = new Set<(r: RunCompleteResult) => void>();
  private readonly errorHandlers = new Set<(e: Error) => void>();

  private waitResolvers: Array<{
    resolve: (r: RunCompleteResult) => void;
    reject: (e: Error) => void;
  }> = [];

  private startedCallback: ((err: Error | null) => void) | null = null;

  constructor(ws: LuftWsClient, initialEventHandler?: (e: AgentEvent) => void) {
    this.ws = ws;
    if (initialEventHandler) {
      this.eventHandlers.add(initialEventHandler);
    }
  }

  get runId(): string {
    return this._runId;
  }

  get done(): boolean {
    return this._done;
  }

  cancel(): void {
    if (this._done) return;
    const msg: RunClientMessage = { type: 'cancel' };
    this.ws.sendJSON(msg);
  }

  onEvent(handler: (event: AgentEvent) => void): () => void {
    this.eventHandlers.add(handler);
    return () => { this.eventHandlers.delete(handler); };
  }

  onComplete(handler: (result: RunCompleteResult) => void): () => void {
    this.completeHandlers.add(handler);
    return () => { this.completeHandlers.delete(handler); };
  }

  onError(handler: (error: Error) => void): () => void {
    this.errorHandlers.add(handler);
    return () => { this.errorHandlers.delete(handler); };
  }

  wait(): Promise<RunCompleteResult> {
    if (this._done) {
      return Promise.resolve({
        runId: this._runId,
        status: this._finalStatus ?? 'unknown',
      });
    }
    return new Promise((resolve, reject) => {
      this.waitResolvers.push({ resolve, reject });
    });
  }

  dispose(): void {
    this._done = true;
    this.eventHandlers.clear();
    this.completeHandlers.clear();
    this.errorHandlers.clear();
    for (const r of this.waitResolvers) {
      r.reject(new Error('RunSession disposed'));
    }
    this.waitResolvers = [];
  }

  // ── Internal ───────────────────────────────────────────────────────────

  private _finalStatus: string | null = null;

  onceStarted(cb: (err: Error | null) => void): void {
    this.startedCallback = cb;
  }

  handleMessage(msg: RunServerMessage): void {
    switch (msg.type) {
      case 'started': {
        this._runId = msg.run_id;
        this.startedCallback?.(null);
        this.startedCallback = null;
        break;
      }
      case 'event': {
        for (const h of this.eventHandlers) {
          try { h(msg.event); } catch { /* swallow */ }
        }
        break;
      }
      case 'complete': {
        this._done = true;
        this._finalStatus = msg.status;
        const result: RunCompleteResult = {
          runId: msg.run_id,
          status: msg.status,
        };
        for (const h of this.completeHandlers) {
          try { h(result); } catch { /* swallow */ }
        }
        for (const r of this.waitResolvers) {
          r.resolve(result);
        }
        this.waitResolvers = [];
        break;
      }
      case 'error': {
        const err = new LuftWsErrorWrapper(msg.message);
        if (!this._runId) {
          this.startedCallback?.(err);
          this.startedCallback = null;
        }
        for (const h of this.errorHandlers) {
          try { h(err); } catch { /* swallow */ }
        }
        if (!this._done) {
          this._done = true;
          this._finalStatus = 'error';
          for (const r of this.waitResolvers) {
            r.reject(err);
          }
          this.waitResolvers = [];
        }
        break;
      }
    }
  }
}

// ── Helpers ───────────────────────────────────────────────────────────────

class LuftWsErrorWrapper extends Error {
  readonly code = 'RUN_SESSION_ERROR';
  constructor(message: string) {
    super(message);
    this.name = 'LuftWsError';
  }
}

export function createRunClient(config: WsClientConfig): RunClient {
  const ws = new LuftWsClient({ reconnect: false, ...config });
  return new RunClient(ws);
}
