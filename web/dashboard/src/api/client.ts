// ============================================================================
// LuftClient — Unified facade combining MCP query API and real-time run
// session streaming.
//
// Manages a persistent MCP WebSocket connection for queries (list runs, get
// status, etc.) and creates ephemeral RunClient instances for each live run
// session.  Exposes both through a single, ergonomic interface.
// ============================================================================

import { LuftWsClient } from './ws-client.js';
import type { WsClientConfig } from './ws-client.js';
import { McpClient } from './mcp-client.js';
import type { McpClientConfig } from './mcp-client.js';
import { RunClient } from './run-client.js';
import type { RunSession, StartOptions } from './run-client.js';
import type {
  ConnectionState,
  ServerInfo,
  ExecuteWorkflowRequest,
  ExecuteWorkflowResponse,
  WorkflowFile,
  ListRunsRequest,
  ListRunsResponse,
  RunStatusResponse,
  GetRunEventsRequest,
  RunEventsResponse,
  CancelRunResponse,
  McpResource,
  ReadResourceResult,
  McpTool,
} from './types.js';

// ── Configuration ─────────────────────────────────────────────────────────

export interface LuftClientConfig {
  url: string;
  backend?: string;
  reconnect?: WsClientConfig['reconnect'];
  reconnectOptions?: WsClientConfig['reconnectOptions'];
  connectTimeoutMs?: number;
  clientInfo?: { name: string; version: string };
  protocolVersion?: string;
}

function buildWsUrl(base: string, path: string, backend?: string): string {
  let url = base.endsWith('/') ? base.slice(0, -1) : base;
  url += path;
  if (backend) {
    url += `?backend=${encodeURIComponent(backend)}`;
  }
  return url;
}

// ── LuftClient ────────────────────────────────────────────────────────────

export class LuftClient {
  private readonly baseUrl: string;
  private readonly backend?: string;
  private readonly mcpWs: LuftWsClient;
  readonly mcp: McpClient;

  constructor(config: LuftClientConfig) {
    this.baseUrl = config.url;
    this.backend = config.backend;

    this.mcpWs = new LuftWsClient({
      url: buildWsUrl(config.url, '/mcp', config.backend),
      reconnect: config.reconnect ?? true,
      reconnectOptions: config.reconnectOptions,
      connectTimeoutMs: config.connectTimeoutMs,
    });

    const mcpConfig: McpClientConfig = {
      ws: this.mcpWs,
      ...(config.clientInfo ? { clientInfo: config.clientInfo } : {}),
      ...(config.protocolVersion ? { protocolVersion: config.protocolVersion } : {}),
    };
    this.mcp = new McpClient(mcpConfig);
  }

  get connectionState(): ConnectionState {
    return this.mcpWs.state;
  }

  get isConnected(): boolean {
    return this.mcpWs.isConnected && this.mcp.initialized;
  }

  get serverInfo(): ServerInfo | null {
    return this.mcp.serverInfo;
  }

  // ── Lifecycle ──────────────────────────────────────────────────────────

  async connect(): Promise<ServerInfo> {
    return this.mcp.connect();
  }

  async disconnect(): Promise<void> {
    this.mcp.disconnect();
  }

  dispose(): void {
    this.mcp.dispose();
  }

  onStateChange(handler: (state: ConnectionState) => void): () => void {
    return this.mcpWs.onStateChange((state) => handler(state));
  }

  // ── MCP proxy (query API) ──────────────────────────────────────────────

  listTools(): Promise<McpTool[]> {
    return this.mcp.listTools();
  }

  listResources(): Promise<McpResource[]> {
    return this.mcp.listResources();
  }

  readResource(uri: string): Promise<ReadResourceResult> {
    return this.mcp.readResource(uri);
  }

  executeWorkflow(req: ExecuteWorkflowRequest): Promise<ExecuteWorkflowResponse> {
    return this.mcp.executeWorkflow(req);
  }

  listFiles(): Promise<WorkflowFile[]> {
    return this.mcp.listFiles();
  }

  listRuns(req?: Partial<ListRunsRequest>): Promise<ListRunsResponse> {
    return this.mcp.listRuns(req);
  }

  getRunStatus(runId: string): Promise<RunStatusResponse> {
    return this.mcp.getRunStatus(runId);
  }

  getRunEvents(runId: string, opts?: Omit<Partial<GetRunEventsRequest>, 'run_id'>): Promise<RunEventsResponse> {
    return this.mcp.getRunEvents(runId, opts);
  }

  cancelRun(runId: string): Promise<CancelRunResponse> {
    return this.mcp.cancelRun(runId);
  }

  // ── Run session streaming ──────────────────────────────────────────────

  createRunSession(options: StartOptions): Promise<RunSession> {
    const runWs = new LuftWsClient({
      url: buildWsUrl(this.baseUrl, '/run', this.backend),
      reconnect: false,
      connectTimeoutMs: 15_000,
    });
    const runClient = new RunClient(runWs);
    return runClient.start(options);
  }
}
