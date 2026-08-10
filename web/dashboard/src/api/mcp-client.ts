// ============================================================================
// McpClient — JSON-RPC 2.0 client for the Luft MCP endpoint (/mcp).
//
// Wraps a LuftWsClient to provide typed, Promise-based access to MCP tools
// (workflow_execute, workflow_status, etc.) and resources.  Handles the
// MCP initialize handshake, request/response correlation by JSON-RPC id,
// and automatic re-initialization after reconnection.
// ============================================================================

import { LuftWsClient } from './ws-client.js';
import type {
  ServerInfo,
  McpTool,
  CallToolResult,
  McpResource,
  ReadResourceResult,
  JsonRpcResponse,
  JsonRpcError,
  JsonRpcRequest,
  JsonRpcNotification,
  ConnectionState,
  ExecuteWorkflowRequest,
  ExecuteWorkflowResponse,
  WorkflowFile,
  ListRunsRequest,
  ListRunsResponse,
  GetRunEventsRequest,
  RunEventsResponse,
  RunStatusResponse,
  CancelRunResponse,
} from './types.js';
import { McpError as McpErrorClass, isJsonRpcResponse } from './types.js';

// ── Types ─────────────────────────────────────────────────────────────────

export interface McpClientConfig {
  ws: LuftWsClient;
  clientInfo?: { name: string; version: string };
  protocolVersion?: string;
}

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

type NotificationHandler = (method: string, params: unknown) => void;

const DEFAULT_PROTOCOL_VERSION = '2024-11-05';
const DEFAULT_CLIENT_INFO = { name: 'luft-dashboard', version: '0.4.6' };
const REQUEST_TIMEOUT_MS = 30_000;

// ── McpClient ─────────────────────────────────────────────────────────────

export class McpClient {
  private readonly ws: LuftWsClient;
  private readonly clientInfo: { name: string; version: string };
  private readonly protocolVersion: string;

  private nextId = 1;
  private readonly pending = new Map<number, PendingRequest>();
  private readonly notificationHandlers = new Set<NotificationHandler>();

  private _initialized = false;
  private _serverInfo: ServerInfo | null = null;
  private _tools: McpTool[] = [];
  private _reconnectInit = false;

  private unsubMessage: (() => void) | null = null;
  private unsubState: (() => void) | null = null;

  constructor(config: McpClientConfig) {
    this.ws = config.ws;
    this.clientInfo = config.clientInfo ?? DEFAULT_CLIENT_INFO;
    this.protocolVersion = config.protocolVersion ?? DEFAULT_PROTOCOL_VERSION;

    this.unsubMessage = this.ws.onMessage((data) => this.handleMessage(data));
    this.unsubState = this.ws.onStateChange((state, prev) => {
      if (state === 'connected' && prev !== 'connected' && this._reconnectInit) {
        this._reconnectInit = false;
        this.initialize().catch(() => { /* error handlers will fire */ });
      }
      if (state === 'reconnecting' || state === 'disconnected') {
        this.failAllPending(new Error('WebSocket disconnected'));
      }
    });
  }

  get initialized(): boolean {
    return this._initialized;
  }

  get serverInfo(): ServerInfo | null {
    return this._serverInfo;
  }

  get tools(): readonly McpTool[] {
    return this._tools;
  }

  get connectionState(): ConnectionState {
    return this.ws.state;
  }

  // ── Lifecycle ──────────────────────────────────────────────────────────

  async initialize(): Promise<ServerInfo> {
    const result = await this.sendRequest<ServerInfo>('initialize', {
      protocolVersion: this.protocolVersion,
      capabilities: {},
      clientInfo: this.clientInfo,
    });

    this._serverInfo = result;
    this._initialized = true;

    this.sendNotification('notifications/initialized');

    try {
      this._tools = await this.listTools();
    } catch {
      // Tool listing is best-effort during init
    }

    return result;
  }

  async connect(): Promise<ServerInfo> {
    await this.ws.connect();
    return this.initialize();
  }

  disconnect(): void {
    this.ws.disconnect();
    this._initialized = false;
  }

  dispose(): void {
    this.failAllPending(new Error('McpClient disposed'));
    this.unsubMessage?.();
    this.unsubState?.();
    this.notificationHandlers.clear();
    this.ws.dispose();
  }

  // ── MCP methods ────────────────────────────────────────────────────────

  async listTools(): Promise<McpTool[]> {
    const result = await this.sendRequest<{ tools: McpTool[] }>('tools/list', {});
    this._tools = result.tools ?? [];
    return this._tools;
  }

  async callTool<T = unknown>(name: string, args?: Record<string, unknown>): Promise<T> {
    const result = await this.sendRequest<CallToolResult>('tools/call', {
      name,
      arguments: args ?? {},
    });

    if (result.isError) {
      const text = result.content?.[0]?.text ?? 'Unknown tool error';
      throw new McpErrorClass(text, -32000, result);
    }

    const text = result.content?.[0]?.text;
    if (text === undefined || text === null) {
      return undefined as T;
    }

    try {
      return JSON.parse(text) as T;
    } catch {
      return text as unknown as T;
    }
  }

  async listResources(): Promise<McpResource[]> {
    const result = await this.sendRequest<{ resources: McpResource[] }>('resources/list', {});
    return result.resources ?? [];
  }

  async readResource(uri: string): Promise<ReadResourceResult> {
    return this.sendRequest<ReadResourceResult>('resources/read', { uri });
  }

  async listResourceTemplates(): Promise<unknown> {
    return this.sendRequest('resources/templates/list', {});
  }

  onNotification(handler: NotificationHandler): () => void {
    this.notificationHandlers.add(handler);
    return () => { this.notificationHandlers.delete(handler); };
  }

  // ── Typed Luft tool wrappers ───────────────────────────────────────────

  async executeWorkflow(req: ExecuteWorkflowRequest): Promise<ExecuteWorkflowResponse> {
    return this.callTool<ExecuteWorkflowResponse>('workflow_execute', req as Record<string, unknown>);
  }

  async listFiles(): Promise<WorkflowFile[]> {
    return this.callTool<WorkflowFile[]>('workflow_list_files');
  }

  async listRuns(req?: Partial<ListRunsRequest>): Promise<ListRunsResponse> {
    return this.callTool<ListRunsResponse>('workflow_list_runs', (req ?? {}) as Record<string, unknown>);
  }

  async getRunStatus(runId: string): Promise<RunStatusResponse> {
    return this.callTool<RunStatusResponse>('workflow_status', { run_id: runId });
  }

  async getRunEvents(runId: string, opts?: Omit<Partial<GetRunEventsRequest>, 'run_id'>): Promise<RunEventsResponse> {
    return this.callTool<RunEventsResponse>('workflow_events', {
      run_id: runId,
      ...opts,
    });
  }

  async cancelRun(runId: string): Promise<CancelRunResponse> {
    return this.callTool<CancelRunResponse>('workflow_cancel', { run_id: runId });
  }

  // ── JSON-RPC plumbing ──────────────────────────────────────────────────

  private sendRequest<T = unknown>(method: string, params?: unknown): Promise<T> {
    const id = this.nextId++;
    const request: JsonRpcRequest = {
      jsonrpc: '2.0',
      id,
      method,
      ...(params !== undefined ? { params } : {}),
    };

    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new McpErrorClass(`Request timeout: ${method} (id=${id})`, -1));
      }, REQUEST_TIMEOUT_MS);

      this.pending.set(id, {
        resolve: resolve as (v: unknown) => void,
        reject,
        timer,
      });

      const sent = this.ws.sendJSON(request);
      if (!sent) {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(new McpErrorClass(`Cannot send: WebSocket not connected (${method})`, -2));
      }
    });
  }

  private sendNotification(method: string, params?: unknown): void {
    const notification: JsonRpcNotification = {
      jsonrpc: '2.0',
      method,
      ...(params !== undefined ? { params } : {}),
    };
    this.ws.sendJSON(notification);
  }

  private handleMessage(data: string): void {
    let msg: unknown;
    try {
      msg = JSON.parse(data);
    } catch {
      return;
    }

    if (isJsonRpcResponse(msg)) {
      this.handleResponse(msg);
    } else if (
      typeof msg === 'object' && msg !== null &&
      'jsonrpc' in msg && !('id' in msg)
    ) {
      const n = msg as JsonRpcNotification;
      for (const h of this.notificationHandlers) {
        try { h(n.method, n.params); } catch { /* swallow */ }
      }
    }
  }

  private handleResponse(msg: JsonRpcResponse): void {
    const pending = this.pending.get(Number(msg.id));
    if (!pending) return;

    clearTimeout(pending.timer);
    this.pending.delete(Number(msg.id));

    if (msg.error) {
      const e = msg.error as JsonRpcError;
      pending.reject(new McpErrorClass(e.message, e.code, e.data));
    } else {
      pending.resolve(msg.result);
    }
  }

  private failAllPending(e: Error): void {
    for (const p of this.pending.values()) {
      clearTimeout(p.timer);
      p.reject(e);
    }
    this.pending.clear();
  }

  // Called by LuftClient when auto-reconnect is desired
  _markReconnectInit(): void {
    this._reconnectInit = true;
  }
}
