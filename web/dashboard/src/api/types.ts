// ============================================================================
// Luft Dashboard API — Protocol Type Definitions
//
// All types mirror the Rust serde serialization in the luft-daemon and
// luft-core crates.  Tag names, casing and optionality are kept in sync
// with the #[serde(...)] attributes on the Rust side.
// ============================================================================

// ── Connection ────────────────────────────────────────────────────────────

export type ConnectionState =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'error';

// ── Core Domain Primitives ────────────────────────────────────────────────

export interface TokenUsage {
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
}

export type RunStatus = 'completed' | 'failed' | 'cancelled' | 'partial';

export type LogLevel = 'trace' | 'debug' | 'info' | 'warn' | 'error';

export type AgentStatus = string;

// ── ProgressDelta  (tag = "kind", rename_all = "snake_case") ───────────────

export type ProgressDelta =
  | { kind: 'message'; text: string }
  | { kind: 'tool_call'; name: string; summary: string }
  | { kind: 'file_edit'; path: string }
  | { kind: 'tokens'; usage: TokenUsage };

// ── Plan Phase ────────────────────────────────────────────────────────────

export interface PlanPhase {
  label: string;
  dynamic?: boolean;
  description?: string | null;
}

// ── Finding ───────────────────────────────────────────────────────────────

export interface FindingLocation {
  file: string;
  line?: number | null;
}

export interface Finding {
  kind: string;
  severity: string;
  title: string;
  detail: string;
  location?: FindingLocation | null;
  evidence: string[];
  data: unknown;
}

// ============================================================================
// Agent Events  (tag = "type", rename_all = "snake_case")
// ============================================================================

export interface RunStartedEvent {
  type: 'run_started';
  run_id: string;
  task: string;
  ts: string;
}

export interface PhaseStartedEvent {
  type: 'phase_started';
  run_id: string;
  phase_id: number;
  label: string;
  planned: number;
  description?: string | null;
  role?: string | null;
  ts?: string | null;
}

export interface AgentStartedEvent {
  type: 'agent_started';
  run_id: string;
  phase_id: number;
  agent_id: string;
  prompt_preview: string;
  model?: string | null;
  description?: string | null;
  role?: string | null;
  name?: string | null;
  agent_seq?: number;
  ts?: string | null;
}

export interface AgentProgressEvent {
  type: 'agent_progress';
  run_id: string;
  agent_id: string;
  delta: ProgressDelta;
}

export interface AcpRawEvent {
  type: 'acp_raw';
  run_id: string;
  agent_id: string;
  kind: string;
  raw: unknown;
}

export interface AcpRequestEvent {
  type: 'acp_request';
  run_id: string;
  agent_id: string;
  method: string;
  raw: unknown;
}

export interface AgentDoneEvent {
  type: 'agent_done';
  run_id: string;
  agent_id: string;
  status: AgentStatus;
  tokens: TokenUsage;
  elapsed_ms: number;
  name?: string | null;
  agent_seq?: number;
  ts?: string | null;
  output?: unknown;
  findings?: Finding[];
  prompt?: string;
  retry_count?: number;
}

export interface PhaseDoneEvent {
  type: 'phase_done';
  run_id: string;
  phase_id: number;
  ok: number;
  failed: number;
  ts?: string | null;
}

export interface RunDoneEvent {
  type: 'run_done';
  run_id: string;
  status: RunStatus;
  total_tokens: TokenUsage;
  report: unknown;
  ts?: string | null;
}

export interface LogEvent {
  type: 'log';
  run_id: string;
  agent_id?: string | null;
  level: LogLevel;
  msg: string;
}

export interface BudgetSetEvent {
  type: 'budget_set';
  run_id: string;
  time_limit_ms?: number | null;
  max_rounds?: number | null;
}

export interface ReportEmittedEvent {
  type: 'report_emitted';
  run_id: string;
  phase_id: number;
  report: unknown;
}

export interface ParallelStartedEvent {
  type: 'parallel_started';
  run_id: string;
  phase_id: number;
  span_id: number;
  count: number;
}

export interface ParallelDoneEvent {
  type: 'parallel_done';
  run_id: string;
  phase_id: number;
  span_id: number;
  ok: number;
  failed: number;
  results: unknown;
  elapsed_ms: number;
}

export interface WorkflowStartedEvent {
  type: 'workflow_started';
  run_id: string;
  span_id: number;
  path: string;
  args: unknown;
}

export interface WorkflowDoneEvent {
  type: 'workflow_done';
  run_id: string;
  span_id: number;
  path: string;
  report: unknown;
  elapsed_ms: number;
  error?: string | null;
}

export interface ConvergeStartedEvent {
  type: 'converge_started';
  run_id: string;
  phase_id: number;
  span_id: number;
  items: number;
  max_rounds: number;
}

export interface ConvergeDoneEvent {
  type: 'converge_done';
  run_id: string;
  phase_id: number;
  span_id: number;
  rounds: number;
  converged: boolean;
  surviving: number;
  result: unknown;
  elapsed_ms: number;
  error?: string | null;
}

export interface PipelineStartedEvent {
  type: 'pipeline_started';
  run_id: string;
  total_stages: number;
  items: number;
}

export interface PipelineStageStartedEvent {
  type: 'pipeline_stage_started';
  run_id: string;
  stage_index: number;
  label: string;
  agents_in_stage: number;
}

export interface PipelineItemDoneEvent {
  type: 'pipeline_item_done';
  run_id: string;
  stage_index: number;
  item_index: number;
  status: AgentStatus;
  tokens: TokenUsage;
  elapsed_ms: number;
}

export interface PipelineDoneEvent {
  type: 'pipeline_done';
  run_id: string;
  stages_completed: number;
  total_ok: number;
  total_failed: number;
}

export interface SchemaRetryEvent {
  type: 'schema_retry';
  run_id: string;
  agent_id: string;
  attempt: number;
  max: number;
}

export interface PlanPreviewEvent {
  type: 'plan_preview';
  run_id: string;
  reasoning: string;
  phases: PlanPhase[];
}

export interface SignalReceivedEvent {
  type: 'signal_received';
  run_id?: string | null;
  signal: string;
  ts: string;
}

export type AgentEvent =
  | RunStartedEvent
  | PhaseStartedEvent
  | AgentStartedEvent
  | AgentProgressEvent
  | AcpRawEvent
  | AcpRequestEvent
  | AgentDoneEvent
  | PhaseDoneEvent
  | RunDoneEvent
  | LogEvent
  | BudgetSetEvent
  | ReportEmittedEvent
  | ParallelStartedEvent
  | ParallelDoneEvent
  | WorkflowStartedEvent
  | WorkflowDoneEvent
  | ConvergeStartedEvent
  | ConvergeDoneEvent
  | PipelineStartedEvent
  | PipelineStageStartedEvent
  | PipelineItemDoneEvent
  | PipelineDoneEvent
  | SchemaRetryEvent
  | PlanPreviewEvent
  | SignalReceivedEvent;

// ============================================================================
// Run Session Protocol  (ws://host/run)
// ============================================================================

export type RunClientMessage =
  | { type: 'start'; script?: string; resume_from_id?: string }
  | { type: 'cancel' };

export type RunServerMessage =
  | { type: 'started'; run_id: string }
  | { type: 'event'; event: AgentEvent }
  | { type: 'complete'; run_id: string; status: string }
  | { type: 'error'; message: string };

// ============================================================================
// MCP Service Request / Response Types
// ============================================================================

export interface ExecuteWorkflowRequest {
  script?: string;
  path?: string;
  resume_from_id?: string;
  args?: unknown;
  concurrency?: number;
  backend?: string;
}

export interface ExecuteWorkflowResponse {
  run_id: string;
  status: string;
  resumed_from: string | null;
}

export interface WorkflowFile {
  name: string;
  path: string;
  description: string;
}

export interface ListRunsRequest {
  limit?: number;
  cursor?: string;
  status_filter?: 'completed' | 'failed' | 'cancelled';
}

export interface RunSummary {
  run_id: string;
  task: string;
  status: string;
  total_tokens: number;
  created_at: string;
  updated_at: string;
}

export interface ListRunsResponse {
  runs: RunSummary[];
  count: number;
  next_cursor: string | null;
  has_more: boolean;
}

export interface GetRunEventsRequest {
  run_id: string;
  since_event_id?: string;
  offset?: number;
  events_limit?: number;
  types?: string[];
  agent_id?: string;
}

export interface RunEventsResponse {
  events: unknown[];
  offset: number;
  events_limit: number;
  total_matching: number;
  next_offset: number | null;
}

export interface CancelRunResponse {
  run_id: string;
  result: string;
  note: string | null;
}

export interface PhaseAgentView {
  short_id: string;
  status: string;
  tokens: number | null;
  findings: number;
  last_message: string | null;
}

export interface PhaseView {
  phase_id: number;
  label: string;
  status: string;
  planned: number | null;
  ok: number;
  failed: number;
  agents: PhaseAgentView[];
}

export interface RunStatusResponse {
  run_id: string;
  run_dir: string;
  task: string;
  status: string;
  current_phase: number;
  completed_phases: number;
  total_started: number;
  completed_agents: number;
  running_agents: number;
  total_tokens: number;
  created_at: string;
  updated_at: string;
  total_phases: number;
  phases: PhaseView[];
  report: unknown;
  error: unknown;
}

// ============================================================================
// JSON-RPC 2.0
// ============================================================================

export interface JsonRpcRequest {
  jsonrpc: '2.0';
  id: number | string;
  method: string;
  params?: unknown;
}

export interface JsonRpcNotification {
  jsonrpc: '2.0';
  method: string;
  params?: unknown;
}

export interface JsonRpcResponse {
  jsonrpc: '2.0';
  id: number | string;
  result?: unknown;
  error?: JsonRpcError;
}

export interface JsonRpcError {
  code: number;
  message: string;
  data?: unknown;
}

// ============================================================================
// MCP Server Info
// ============================================================================

export interface ServerInfo {
  protocolVersion: string;
  capabilities: {
    tools?: unknown;
    resources?: unknown;
    [key: string]: unknown;
  };
  serverInfo: {
    name: string;
    title?: string;
    version: string;
  };
  instructions?: string;
}

export interface McpTool {
  name: string;
  description?: string;
  inputSchema: {
    type: string;
    properties?: Record<string, unknown>;
    [key: string]: unknown;
  };
}

export interface CallToolResult {
  content: Array<{
    type: string;
    text?: string;
    [key: string]: unknown;
  }>;
  isError?: boolean;
}

export interface McpResource {
  uri: string;
  name: string;
  title?: string;
  description?: string;
  mimeType?: string;
  size?: number;
}

export interface ReadResourceResult {
  contents: Array<{
    uri: string;
    mimeType?: string;
    text?: string;
    meta?: unknown;
  }>;
}

// ============================================================================
// Errors
// ============================================================================

export class LuftWsError extends Error {
  readonly code: string;
  readonly cause?: unknown;

  constructor(message: string, code: string, cause?: unknown) {
    super(message);
    this.name = 'LuftWsError';
    this.code = code;
    if (cause !== undefined) this.cause = cause;
  }
}

export class McpError extends Error {
  readonly code: number;
  readonly data?: unknown;

  constructor(message: string, code: number, data?: unknown) {
    super(message);
    this.name = 'McpError';
    this.code = code;
    if (data !== undefined) this.data = data;
  }
}

// ── Type guards ───────────────────────────────────────────────────────────

export function isRunServerMessage(v: unknown): v is RunServerMessage {
  if (typeof v !== 'object' || v === null) return false;
  const t = (v as { type?: unknown }).type;
  return t === 'started' || t === 'event' || t === 'complete' || t === 'error';
}

export function isJsonRpcResponse(v: unknown): v is JsonRpcResponse {
  if (typeof v !== 'object' || v === null) return false;
  const o = v as Record<string, unknown>;
  return o.jsonrpc === '2.0' && ('result' in o || 'error' in o);
}
