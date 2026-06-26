export type RunId = string
export type AgentId = string
export type PhaseId = number

export interface TokenUsage {
  input: number
  output: number
}

export type RunStatus = 'running' | 'completed' | 'failed' | 'cancelled'
export type AgentStatus = 'pending' | 'running' | 'done' | 'failed'
export type AgentRole = 'producer' | 'adversary' | 'voter' | 'default'

export interface RunSummary {
  run_id: RunId
  run_dir: string
  task: string
  status: RunStatus
  current_phase: number
  total_phases: number
  total_tokens: number
  started_at: string
  elapsed_ms: number
}

export interface RunsResponse {
  runs: RunSummary[]
  total: number
}

export interface PhaseSummary {
  phase_id: PhaseId
  label: string
  status: AgentStatus | 'pending'
  role: AgentRole
  planned: number
  ok: number
  failed: number
}

export interface AgentResultCache {
  agent_id: AgentId
  role: AgentRole
  status: AgentStatus
  tokens: TokenUsage
  elapsed_ms: number
  prompt_preview: string
  output_preview: string
  description?: string
  tool_calls: number
  error?: string
}

export interface Finding {
  id: string
  severity: 'low' | 'medium' | 'high' | 'critical'
  message: string
  source?: AgentId
}

export interface PhaseDetail {
  phase_id: PhaseId
  label: string
  description?: string
  role: AgentRole
  status: 'completed' | 'running' | 'pending'
  agents: AgentResultCache[]
}

export interface RunCheckpoint {
  run_id: RunId
  task: string
  status: RunStatus
  current_phase: number
  phases: PhaseDetail[]
  findings: Finding[]
  total_tokens: number
  total_tokens_detail: TokenUsage
  started_at: string
  elapsed_ms: number
}

export interface ProgressDelta {
  tokens?: Partial<TokenUsage>
  content?: string
}

export type AgentEvent =
  | { type: 'RunStarted'; run_id: RunId; task: string; ts: string }
  | { type: 'PhaseStarted'; run_id: RunId; phase_id: PhaseId; label: string; planned: number; ts: string }
  | { type: 'AgentStarted'; run_id: RunId; phase_id: PhaseId; agent_id: AgentId; role: AgentRole; prompt_preview: string; model?: string; ts: string }
  | { type: 'AgentProgress'; run_id: RunId; agent_id: AgentId; delta: ProgressDelta; ts: string }
  | { type: 'AgentDone'; run_id: RunId; agent_id: AgentId; status: AgentStatus; tokens: TokenUsage; elapsed_ms: number; ts: string }
  | { type: 'PhaseDone'; run_id: RunId; phase_id: PhaseId; ok: number; failed: number; ts: string }
  | { type: 'RunDone'; run_id: RunId; status: RunStatus; total_tokens: TokenUsage; ts: string }

export interface StartRunRequest {
  workflow: string
  task: string
  backend: string
}

export interface StartRunResponse {
  run_id: RunId
  status: 'running'
  ws_url: string
}

export interface DashboardStats {
  today_runs: number
  today_tokens: number
  today_success: number
  today_failed: number
  active_runs: RunSummary[]
  recent_runs: RunSummary[]
}

export interface WorkflowSummary {
  name: string
  description: string
  phases: number
  agents: number
}

export interface WorkflowDetail {
  name: string
  content: string
  description: string
}

export interface BackendConfig {
  id: string
  name: string
  provider: string
  model: string
  connected: boolean
  usage_count: number
}

export interface RunFilters {
  status?: RunStatus | 'all'
  time?: 'today' | '24h' | '7d' | 'all'
  q?: string
}
