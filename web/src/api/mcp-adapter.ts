// ============================================================================
// MCP Adapter — LuftAPI implementation backed by the MCP WebSocket + HTTP REST.
//
// Uses the @luft/dashboard-api package (LuftClient, McpClient, RunClient) for
// MCP operations and the existing httpClient for HTTP GET endpoints.
// Connects lazily (on first use) and auto-reconnects via LuftClient.
// ============================================================================

import { httpClient } from './http-client'
import { endpoints } from './endpoints'
import { LuftClient } from '@luft/dashboard-api'
import type {
  RunStatusResponse,
  PhaseView,
  PhaseAgentView,
  WorkflowFile,
  ServerInfo,
  ListRunsResponse,
  ExecuteWorkflowResponse,
  RunSummary as McpRunSummary,
} from '@luft/dashboard-api'
import type { LuftAPI } from './types-api'
import type {
  RunsResponse,
  RunCheckpoint,
  AgentEvent,
  DashboardStats,
  WorkflowSummary,
  WorkflowDetail,
  BackendConfig,
  RunFilters,
  StartRunRequest,
  StartRunResponse,
  CancelRunResponse,
  BackendTestResponse,
  RunLogsRequest,
  RunLogsResponse,
  RunArtifactsResponse,
  CreateBackendRequest,
  CreateBackendResponse,
  DeleteBackendResponse,
  WorkflowSaveRequest,
  WorkflowRunRequest,
  RunStatus,
  PhaseDetail,
  AgentResultCache,
  LogLine,
} from './types'
import type { ApiHealth, ApiVersionInfo } from './common'

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

/**
 * Build the WebSocket base URL from the page's current location (same host:port).
 */
function getWsBaseUrl(): string {
  const proto = globalThis.location?.protocol === 'https:' ? 'wss:' : 'ws:'
  const host = globalThis.location?.host ?? 'localhost:5173'
  return `${proto}//${host}`
}

// ── Status mapping ────────────────────────────────────────────────────────

/**
 * Map MCP-style status strings to the frontend RunStatus union.
 * MCP uses 'partial' for in-progress runs; frontend expects 'running'.
 */
function mapRunStatus(status: string): RunStatus {
  if (status === 'partial') return 'running'
  if (status === 'completed' || status === 'failed' || status === 'cancelled') {
    return status as RunStatus
  }
  // Treat any unknown status as 'running' (e.g. 'created', 'queued')
  return 'running'
}

/**
 * Map MCP agent status strings to the frontend AgentStatus.
 */
function mapAgentStatus(status: string): AgentResultCache['status'] {
  if (status === 'done' || status === 'running' || status === 'pending' || status === 'failed') {
    return status
  }
  if (status === 'completed') return 'done'
  return 'pending'
}

// ── Type converters ───────────────────────────────────────────────────────

function convertPhaseAgentView(pav: PhaseAgentView): AgentResultCache {
  return {
    agent_id: pav.short_id,
    role: 'default',
    status: mapAgentStatus(pav.status),
    tokens: { input: 0, output: pav.tokens ?? 0 },
    elapsed_ms: 0,
    prompt_preview: pav.last_message ?? '',
    output_preview: '',
    tool_calls: 0,
    error: pav.status === 'failed' ? pav.last_message ?? undefined : undefined,
  }
}

function convertPhaseView(pv: PhaseView): PhaseDetail {
  let status: PhaseDetail['status'] = 'pending'
  if (pv.status === 'completed') status = 'completed'
  else if (pv.status === 'running') status = 'running'

  return {
    phase_id: pv.phase_id,
    label: pv.label,
    role: 'default',
    status,
    agents: pv.agents.map(convertPhaseAgentView),
  }
}

function convertRunStatusResponse(resp: RunStatusResponse): RunCheckpoint {
  const elapsed_ms = resp.created_at
    ? Date.now() - new Date(resp.created_at).getTime()
    : 0

  return {
    run_id: resp.run_id,
    task: resp.task,
    status: mapRunStatus(resp.status),
    current_phase: resp.current_phase,
    phases: (resp.phases ?? []).map(convertPhaseView),
    findings: [],
    total_tokens: resp.total_tokens,
    total_tokens_detail: { input: 0, output: resp.total_tokens },
    started_at: resp.created_at,
    elapsed_ms,
  }
}

function convertMcpRunSummary(rs: McpRunSummary): RunsResponse['runs'][number] {
  const elapsed_ms = rs.created_at
    ? Date.now() - new Date(rs.created_at).getTime()
    : 0

  return {
    run_id: rs.run_id,
    run_dir: rs.run_id,
    task: rs.task,
    status: mapRunStatus(rs.status),
    current_phase: 0,
    total_phases: 0,
    total_tokens: rs.total_tokens,
    started_at: rs.created_at,
    elapsed_ms,
  }
}

function convertWorkflowFile(wf: WorkflowFile): WorkflowSummary {
  return {
    name: wf.name,
    description: wf.description,
    phases: 0,
    agents: 0,
  }
}

/**
 * Convert MCP AgentEvent (snake_case) to frontend AgentEvent (PascalCase types).
 */
function convertMcpEvent(mcpEvent: unknown): AgentEvent {
  const ev = mcpEvent as Record<string, unknown>
  const type = ev.type as string

  // Map snake_case MCP event type to PascalCase frontend type
  const typeMap: Record<string, string> = {
    run_started: 'RunStarted',
    phase_started: 'PhaseStarted',
    agent_started: 'AgentStarted',
    agent_progress: 'AgentProgress',
    acp_request: 'AcpRequest',
    agent_done: 'AgentDone',
    phase_done: 'PhaseDone',
    run_done: 'RunDone',
  }

  const mappedType = typeMap[type] ?? type

  // Build a normalized event — the frontend type guard is lenient and only
  // checks for type, run_id, and ts fields.
  const base = {
    type: mappedType,
    run_id: (ev.run_id ?? '') as string,
    ts: (ev.ts ?? new Date().toISOString()) as string,
  }

  // Copy all remaining fields through
  const result: Record<string, unknown> = { ...base }
  for (const [key, value] of Object.entries(ev)) {
    if (key === 'type' || key === 'run_id' || key === 'ts') continue
    result[key] = value
  }

  return result as AgentEvent
}

/**
 * Convert MCP events into LogLine[] for getLogs.
 */
function extractLogLinesFromEvents(events: unknown[]): LogLine[] {
  const lines: LogLine[] = []

  for (const ev of events) {
    const e = ev as Record<string, unknown>
    if (e.type === 'log' || e.type === 'LogEvent') {
      lines.push({
        ts: (e.ts as string) ?? new Date().toISOString(),
        level: mapLogLevel(e.level as string | undefined),
        message: (e.msg as string) ?? (e.message as string) ?? '',
        agent_id: (e.agent_id as string | undefined) ?? undefined,
      })
    }
  }

  return lines
}

function mapLogLevel(level?: string): LogLine['level'] {
  if (level === 'debug' || level === 'info' || level === 'warn' || level === 'error') {
    return level
  }
  return 'info'
}

// ═══════════════════════════════════════════════════════════════════════════
// MCP connection singleton
// ═══════════════════════════════════════════════════════════════════════════

let _mcpClient: LuftClient | null = null
let _mcpConnectPromise: Promise<ServerInfo> | null = null
let _mcpServerInfo: ServerInfo | null = null

/**
 * Get (or create) the lazy MCP client singleton.
 * Connection is established on first use.
 */
async function getMcpClient(): Promise<LuftClient> {
  if (_mcpClient && _mcpClient.isConnected) {
    return _mcpClient
  }

  if (!_mcpClient) {
    _mcpClient = new LuftClient({
      url: getWsBaseUrl(),
      reconnect: true,
      reconnectOptions: {
        maxRetries: 10,
        initialDelayMs: 1000,
        maxDelayMs: 30_000,
      },
      clientInfo: { name: 'luft-web', version: '0.4.6' },
    })
  }

  if (!_mcpConnectPromise) {
    _mcpConnectPromise = _mcpClient.connect().then((info: ServerInfo) => {
      _mcpServerInfo = info
      return info
    })
  }

  try {
    const promise = _mcpConnectPromise
    await promise
    return _mcpClient!
  } catch (err) {
    // Reset so next call retries fresh
    _mcpConnectPromise = null
    _mcpClient = null
    throw err
  }
}

/**
 * Reset the MCP client singleton (e.g. on auth change or forced disconnect).
 */
export function resetMcpClient(): void {
  _mcpClient?.dispose()
  _mcpClient = null
  _mcpConnectPromise = null
  _mcpServerInfo = null
}

// ═══════════════════════════════════════════════════════════════════════════
// Adapter implementation
// ═══════════════════════════════════════════════════════════════════════════

function buildRunFilters(filters?: RunFilters): Record<string, string | undefined> {
  const params: Record<string, string | undefined> = {}
  if (filters?.status && filters.status !== 'all') params.status = filters.status
  if (filters?.time && filters.time !== 'all') params.time = filters.time
  if (filters?.q) params.q = filters.q
  return params
}

export const mcpAdapter: LuftAPI = {
  // ── Runs ────────────────────────────────────────────────────────────────

  runs: {
    /**
     * List runs via HTTP GET /api/runs.
     * The backend serves this endpoint via the REST API.
     */
    async list(filters?: RunFilters): Promise<RunsResponse> {
      return httpClient.get<RunsResponse>(endpoints.runs.list, {
        params: buildRunFilters(filters),
      })
    },

    /**
     * Get run checkpoint via MCP workflow_status.
     */
    async get(runId: string): Promise<RunCheckpoint> {
      const client = await getMcpClient()
      const status = await client.getRunStatus(runId)
      return convertRunStatusResponse(status)
    },

    /**
     * Get run events via MCP workflow_events.
     */
    async getEvents(runId: string): Promise<AgentEvent[]> {
      const client = await getMcpClient()
      const resp = await client.getRunEvents(runId)
      return (resp.events ?? []).map(convertMcpEvent)
    },

    /**
     * Start a run via MCP workflow_execute.
     */
    async start(req: StartRunRequest): Promise<StartRunResponse> {
      const client = await getMcpClient()
      const result: ExecuteWorkflowResponse = await client.executeWorkflow({
        path: req.workflow,
        args: { task: req.task },
        ...(req.backend ? { backend: req.backend } : {}),
      })
      return {
        run_id: result.run_id,
        status: 'running',
        ws_url: `${getWsBaseUrl()}/run`,
      }
    },

    /**
     * Cancel a run via MCP workflow_cancel.
     */
    async cancel(runId: string): Promise<CancelRunResponse> {
      const client = await getMcpClient()
      await client.cancelRun(runId)
      return {
        run_id: runId,
        status: 'cancelled',
        cancelled_at: new Date().toISOString(),
      }
    },

    /**
     * Get run logs — extracted from MCP events (LogEvent type).
     */
    async getLogs(req: RunLogsRequest): Promise<RunLogsResponse> {
      const client = await getMcpClient()
      const resp = await client.getRunEvents(req.run_id, {
        types: ['log'],
        events_limit: req.tail ?? 100,
      })
      const lines = extractLogLinesFromEvents(resp.events)

      // Apply level filter
      let filtered = lines
      if (req.level) {
        const levels = ['debug', 'info', 'warn', 'error']
        const minIdx = levels.indexOf(req.level)
        if (minIdx >= 0) {
          filtered = filtered.filter((l) => levels.indexOf(l.level) >= minIdx)
        }
      }

      // Apply tail
      if (req.tail && req.tail > 0 && filtered.length > req.tail) {
        filtered = filtered.slice(-req.tail)
      }

      return {
        run_id: req.run_id,
        lines: filtered,
        has_more: resp.next_offset !== null && resp.next_offset > 0,
      }
    },

    /**
     * Get run artifacts — not supported via MCP or REST.
     */
    async getArtifacts(_runId: string): Promise<RunArtifactsResponse> {
      throw new Error(
        'Artifacts are not available via the current backend. ' +
          'The backend does not expose a file artifact API.',
      )
    },
  },

  // ── Stats ───────────────────────────────────────────────────────────────

  stats: {
    /**
     * Compute dashboard stats by aggregating MCP workflow_list_runs.
     */
    async get(): Promise<DashboardStats> {
      const client = await getMcpClient()
      const allRuns: ListRunsResponse = await client.listRuns({ limit: 100 })

      const todayStart = new Date()
      todayStart.setHours(0, 0, 0, 0)
      const todayMs = todayStart.getTime()

      let todayRuns = 0
      let todayTokens = 0
      let todaySuccess = 0
      let todayFailed = 0
      const activeRuns: RunsResponse['runs'] = []
      const recentRuns: RunsResponse['runs'] = []

      for (const run of allRuns.runs) {
        const created = run.created_at ? new Date(run.created_at).getTime() : 0
        const isToday = created >= todayMs

        const summary = convertMcpRunSummary(run)

        if (isToday) {
          todayRuns++
          todayTokens += run.total_tokens
          if (run.status === 'completed' || run.status === 'done') todaySuccess++
          else if (run.status === 'failed' || run.status === 'cancelled') todayFailed++
        }

        if (run.status === 'partial' || run.status === 'running') {
          activeRuns.push(summary)
        }

        recentRuns.push(summary)
      }

      // Sort recent runs by newest first, limit to 10
      recentRuns.sort(
        (a, b) => new Date(b.started_at).getTime() - new Date(a.started_at).getTime(),
      )

      return {
        today_runs: todayRuns,
        today_tokens: todayTokens,
        today_success: todaySuccess,
        today_failed: todayFailed,
        active_runs: activeRuns.slice(0, 10),
        recent_runs: recentRuns.slice(0, 10),
      }
    },
  },

  // ── System ──────────────────────────────────────────────────────────────

  system: {
    /**
     * Health check via HTTP GET /api/health.
     */
    async health(): Promise<ApiHealth> {
      return httpClient.get<ApiHealth>(endpoints.health)
    },

    /**
     * Version info from the MCP server info (initialize handshake).
     */
    async version(): Promise<ApiVersionInfo> {
      const client = await getMcpClient()
      const info = client.serverInfo ?? _mcpServerInfo
      if (!info) {
        throw new Error('Server info not available — MCP client not initialized')
      }
      return {
        api_version: info.serverInfo.version,
        build_version: info.serverInfo.name,
        features: [],
      }
    },
  },

  // ── Workflows ───────────────────────────────────────────────────────────

  workflows: {
    /**
     * List workflow files via MCP workflow_list_files.
     */
    async list(): Promise<WorkflowSummary[]> {
      const client = await getMcpClient()
      const files: WorkflowFile[] = await client.listFiles()
      return files.map(convertWorkflowFile)
    },

    /**
     * Get workflow detail via MCP readResource(workflow://{name}).
     */
    async get(name: string): Promise<WorkflowDetail> {
      const client = await getMcpClient()
      const resource = await client.readResource(`workflow://${name}`)
      const text = resource.contents?.[0]?.text ?? ''
      return {
        name,
        content: text,
        description: '',
        last_modified: undefined,
      }
    },

    /**
     * Save a workflow — not supported by the current backend.
     */
    async save(_req: WorkflowSaveRequest): Promise<WorkflowDetail> {
      throw new Error(
        'Workflow save is not supported by the current backend. ' +
          'The MCP API does not expose a workflow write endpoint. ' +
          'Edit workflow files directly on the server.',
      )
    },

    /**
     * Delete a workflow — not supported by the current backend.
     */
    async delete(_name: string): Promise<void> {
      throw new Error(
        'Workflow deletion is not supported by the current backend. ' +
          'The MCP API does not expose a workflow delete endpoint. ' +
          'Remove workflow files directly on the server.',
      )
    },

    /**
     * Run a workflow via MCP workflow_execute with path=name.
     */
    async run(req: WorkflowRunRequest): Promise<StartRunResponse> {
      const client = await getMcpClient()
      const result: ExecuteWorkflowResponse = await client.executeWorkflow({
        path: req.name,
        args: req.task ? { topic: req.task } : undefined,
        ...(req.backend ? { backend: req.backend } : {}),
      })
      return {
        run_id: result.run_id,
        status: 'running',
        ws_url: `${getWsBaseUrl()}/run`,
      }
    },
  },

  // ── Backends ────────────────────────────────────────────────────────────

  backends: {
    /**
     * Backend management is not available via the current backend.
     */
    async list(): Promise<BackendConfig[]> {
      throw new Error(
        'Backend management is not available via the current backend. ' +
          'The MCP and REST APIs do not expose backend configuration endpoints.',
      )
    },

    /**
     * Backend test — not supported.
     */
    async test(_id: string): Promise<BackendTestResponse> {
      throw new Error(
        'Backend testing is not available via the current backend. ' +
          'The MCP and REST APIs do not expose backend test endpoints.',
      )
    },

    /**
     * Backend creation — not supported.
     */
    async create(_req: CreateBackendRequest): Promise<CreateBackendResponse> {
      throw new Error(
        'Backend creation is not available via the current backend. ' +
          'The MCP and REST APIs do not expose backend configuration endpoints.',
      )
    },

    /**
     * Backend deletion — not supported.
     */
    async delete(_id: string): Promise<DeleteBackendResponse> {
      throw new Error(
        'Backend deletion is not available via the current backend. ' +
          'The MCP and REST APIs do not expose backend configuration endpoints.',
      )
    },
  },
}

export default mcpAdapter