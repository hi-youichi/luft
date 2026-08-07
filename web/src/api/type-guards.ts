import type {
  RunStatus,
  AgentStatus,
  AgentRole,
  TokenUsage,
  RunSummary,
  PhaseSummary,
  AgentResultCache,
  Finding,
  PhaseDetail,
  RunCheckpoint,
  AgentEvent,
  StartRunResponse,
  DashboardStats,
  WorkflowSummary,
  WorkflowDetail,
  BackendConfig,
  CancelRunResponse,
  BackendTestResponse,
  RunLogsResponse,
  LogLine,
  RunArtifact,
  RunArtifactsResponse,
  CreateBackendResponse,
  DeleteBackendResponse,
} from './types'
import type { ApiHealth, ApiVersionInfo } from './common'

const RUN_STATUSES = new Set<RunStatus>(['running', 'completed', 'failed', 'cancelled'])
const AGENT_STATUSES = new Set<AgentStatus>(['pending', 'running', 'done', 'failed'])
const AGENT_ROLES = new Set<AgentRole>(['producer', 'adversary', 'voter', 'default'])
const SEVERITIES = new Set(['low', 'medium', 'high', 'critical'])
const LOG_LEVELS = new Set(['debug', 'info', 'warn', 'error'])

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

function isString(v: unknown): v is string {
  return typeof v === 'string'
}

function isNumber(v: unknown): v is number {
  return typeof v === 'number' && Number.isFinite(v)
}

function isOptional<T>(v: unknown, check: (v: unknown) => v is T): v is T | undefined {
  return v === undefined || check(v)
}

export function isTokenUsage(v: unknown): v is TokenUsage {
  return isObject(v) && isNumber(v.input) && isNumber(v.output)
}

export function isRunStatus(v: unknown): v is RunStatus {
  return typeof v === 'string' && RUN_STATUSES.has(v as RunStatus)
}

export function isAgentStatus(v: unknown): v is AgentStatus {
  return typeof v === 'string' && AGENT_STATUSES.has(v as AgentStatus)
}

export function isAgentRole(v: unknown): v is AgentRole {
  return typeof v === 'string' && AGENT_ROLES.has(v as AgentRole)
}

export function isRunSummary(v: unknown): v is RunSummary {
  if (!isObject(v)) return false
  return (
    isString(v.run_id) &&
    isString(v.run_dir) &&
    isString(v.task) &&
    isRunStatus(v.status) &&
    isNumber(v.current_phase) &&
    isNumber(v.total_phases) &&
    isNumber(v.total_tokens) &&
    isString(v.started_at) &&
    isNumber(v.elapsed_ms)
  )
}

export function isPhaseSummary(v: unknown): v is PhaseSummary {
  if (!isObject(v)) return false
  return (
    isNumber(v.phase_id) &&
    isString(v.label) &&
    (isAgentStatus(v.status) || v.status === 'pending') &&
    isAgentRole(v.role) &&
    isNumber(v.planned) &&
    isNumber(v.ok) &&
    isNumber(v.failed)
  )
}

export function isAgentResultCache(v: unknown): v is AgentResultCache {
  if (!isObject(v)) return false
  return (
    isString(v.agent_id) &&
    isAgentRole(v.role) &&
    isAgentStatus(v.status) &&
    isTokenUsage(v.tokens) &&
    isNumber(v.elapsed_ms) &&
    isString(v.prompt_preview) &&
    isString(v.output_preview) &&
    isOptional(v.description, isString) &&
    isNumber(v.tool_calls) &&
    isOptional(v.error, isString)
  )
}

export function isFinding(v: unknown): v is Finding {
  if (!isObject(v)) return false
  return (
    isString(v.id) &&
    typeof v.severity === 'string' &&
    SEVERITIES.has(v.severity) &&
    isString(v.message) &&
    isOptional(v.source, isString)
  )
}

export function isPhaseDetail(v: unknown): v is PhaseDetail {
  if (!isObject(v)) return false
  return (
    isNumber(v.phase_id) &&
    isString(v.label) &&
    isOptional(v.description, isString) &&
    isAgentRole(v.role) &&
    (v.status === 'completed' || v.status === 'running' || v.status === 'pending') &&
    Array.isArray(v.agents) &&
    v.agents.every(isAgentResultCache)
  )
}

export function isRunCheckpoint(v: unknown): v is RunCheckpoint {
  if (!isObject(v)) return false
  return (
    isString(v.run_id) &&
    isString(v.task) &&
    isRunStatus(v.status) &&
    isNumber(v.current_phase) &&
    Array.isArray(v.phases) &&
    v.phases.every(isPhaseDetail) &&
    Array.isArray(v.findings) &&
    v.findings.every(isFinding) &&
    isNumber(v.total_tokens) &&
    isTokenUsage(v.total_tokens_detail) &&
    isString(v.started_at) &&
    isNumber(v.elapsed_ms)
  )
}

const AGENT_EVENT_TYPES = new Set([
  'RunStarted', 'PhaseStarted', 'AgentStarted', 'AgentProgress',
  'AcpRequest', 'AgentDone', 'PhaseDone', 'RunDone',
])

export function isAgentEvent(v: unknown): v is AgentEvent {
  if (!isObject(v)) return false
  return (
    typeof v.type === 'string' &&
    AGENT_EVENT_TYPES.has(v.type) &&
    isString(v.run_id) &&
    isString(v.ts)
  )
}

export function isStartRunResponse(v: unknown): v is StartRunResponse {
  if (!isObject(v)) return false
  return (
    isString(v.run_id) &&
    v.status === 'running' &&
    isString(v.ws_url)
  )
}

export function isCancelRunResponse(v: unknown): v is CancelRunResponse {
  if (!isObject(v)) return false
  return (
    isString(v.run_id) &&
    v.status === 'cancelled' &&
    isString(v.cancelled_at)
  )
}

export function isDashboardStats(v: unknown): v is DashboardStats {
  if (!isObject(v)) return false
  return (
    isNumber(v.today_runs) &&
    isNumber(v.today_tokens) &&
    isNumber(v.today_success) &&
    isNumber(v.today_failed) &&
    Array.isArray(v.active_runs) &&
    v.active_runs.every(isRunSummary) &&
    Array.isArray(v.recent_runs) &&
    v.recent_runs.every(isRunSummary)
  )
}

export function isWorkflowSummary(v: unknown): v is WorkflowSummary {
  if (!isObject(v)) return false
  return (
    isString(v.name) &&
    isString(v.description) &&
    isNumber(v.phases) &&
    isNumber(v.agents)
  )
}

export function isWorkflowDetail(v: unknown): v is WorkflowDetail {
  if (!isObject(v)) return false
  return (
    isString(v.name) &&
    isString(v.content) &&
    isString(v.description)
  )
}

export function isBackendConfig(v: unknown): v is BackendConfig {
  if (!isObject(v)) return false
  return (
    isString(v.id) &&
    isString(v.name) &&
    isString(v.provider) &&
    isString(v.model) &&
    typeof v.connected === 'boolean' &&
    isNumber(v.usage_count)
  )
}

export function isBackendTestResponse(v: unknown): v is BackendTestResponse {
  if (!isObject(v)) return false
  return (
    isString(v.id) &&
    typeof v.connected === 'boolean' &&
    isOptional(v.latency_ms, isNumber) &&
    isOptional(v.error, isString)
  )
}

export function isLogLine(v: unknown): v is LogLine {
  if (!isObject(v)) return false
  return (
    isString(v.ts) &&
    typeof v.level === 'string' &&
    LOG_LEVELS.has(v.level) &&
    isString(v.message) &&
    isOptional(v.agent_id, isString)
  )
}

export function isRunLogsResponse(v: unknown): v is RunLogsResponse {
  if (!isObject(v)) return false
  return (
    isString(v.run_id) &&
    Array.isArray(v.lines) &&
    v.lines.every(isLogLine) &&
    typeof v.has_more === 'boolean'
  )
}

export function isRunArtifact(v: unknown): v is RunArtifact {
  if (!isObject(v)) return false
  return (
    isString(v.run_id) &&
    isString(v.name) &&
    isString(v.path) &&
    isNumber(v.size) &&
    isString(v.mime_type) &&
    isString(v.created_at)
  )
}

export function isRunArtifactsResponse(v: unknown): v is RunArtifactsResponse {
  if (!isObject(v)) return false
  return (
    isString(v.run_id) &&
    Array.isArray(v.artifacts) &&
    v.artifacts.every(isRunArtifact)
  )
}

export function isCreateBackendResponse(v: unknown): v is CreateBackendResponse {
  if (!isObject(v)) return false
  return isBackendConfig(v.backend)
}

export function isDeleteBackendResponse(v: unknown): v is DeleteBackendResponse {
  if (!isObject(v)) return false
  return isString(v.id) && typeof v.deleted === 'boolean'
}

export function isApiHealth(v: unknown): v is ApiHealth {
  if (!isObject(v)) return false
  return (
    (v.status === 'ok' || v.status === 'degraded' || v.status === 'down') &&
    isString(v.version) &&
    isNumber(v.uptime_ms) &&
    Array.isArray(v.checks)
  )
}

export function isApiVersionInfo(v: unknown): v is ApiVersionInfo {
  if (!isObject(v)) return false
  return (
    isString(v.api_version) &&
    isString(v.build_version) &&
    isOptional(v.git_commit, isString) &&
    isOptional(v.build_date, isString) &&
    Array.isArray(v.features) &&
    v.features.every(isString)
  )
}
