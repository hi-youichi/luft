import { useQuery } from '@tanstack/react-query'
import { useCallback, useEffect, useMemo, useReducer } from 'react'
import { api, queryKeys } from '@/api'
import { useLiveEvents } from '@/hooks/useLiveEvents'
import type {
  AgentEvent, AgentResultCache, AgentStatus, PhaseDetail, RunCheckpoint,
} from '@/api/types'

type CheckpointAction =
  | { type: 'SET_CHECKPOINT'; payload: RunCheckpoint }
  | { type: 'APPLY_EVENT'; payload: AgentEvent }

function updateAgentInPhase(
  phases: PhaseDetail[],
  agentId: string,
  updater: (a: AgentResultCache) => AgentResultCache,
): PhaseDetail[] {
  return phases.map((phase) => ({
    ...phase,
    agents: phase.agents.map((agent) =>
      agent.agent_id === agentId ? updater(agent) : agent,
    ),
  }))
}

function applyEvent(state: RunCheckpoint, event: AgentEvent): RunCheckpoint {
  switch (event.type) {
    case 'PhaseStarted':
      return {
        ...state,
        current_phase: Math.max(state.current_phase, event.phase_id),
        phases: state.phases.map((p) =>
          p.phase_id === event.phase_id
            ? { ...p, status: 'running' }
            : p,
        ),
      }

    case 'AgentStarted': {
      const existing = state.phases
        .find((p) => p.phase_id === event.phase_id)
        ?.agents.some((a) => a.agent_id === event.agent_id)

      if (existing) {
        return {
          ...state,
          phases: updateAgentInPhase(state.phases, event.agent_id, (a) => ({
            ...a,
            status: 'running' as AgentStatus,
            prompt_preview: event.prompt_preview,
          })),
        }
      }

      return {
        ...state,
        phases: state.phases.map((p) =>
          p.phase_id === event.phase_id
            ? {
                ...p,
                agents: [
                  ...p.agents,
                  {
                    agent_id: event.agent_id,
                    role: event.role,
                    status: 'running' as AgentStatus,
                    tokens: { input: 0, output: 0 },
                    elapsed_ms: 0,
                    prompt_preview: event.prompt_preview,
                    output_preview: '',
                    tool_calls: 0,
                  },
                ],
              }
            : p,
        ),
      }
    }

    case 'AgentProgress':
      return {
        ...state,
        phases: updateAgentInPhase(state.phases, event.agent_id, (a) => {
          const input = event.delta.tokens?.input ?? 0
          const output = event.delta.tokens?.output ?? 0
          return {
            ...a,
            tokens: {
              input: a.tokens.input + input,
              output: a.tokens.output + output,
            },
            ...(event.delta.content != null && { output_preview: event.delta.content }),
          }
        }),
      }

    case 'AgentDone':
      return {
        ...state,
        phases: updateAgentInPhase(state.phases, event.agent_id, (a) => ({
          ...a,
          status: event.status,
          tokens: event.tokens,
          elapsed_ms: event.elapsed_ms,
        })),
      }

    case 'PhaseDone':
      return {
        ...state,
        phases: state.phases.map((p) =>
          p.phase_id === event.phase_id
            ? { ...p, status: 'completed' }
            : p,
        ),
      }

    case 'RunDone':
      return {
        ...state,
        status: event.status,
        total_tokens_detail: event.total_tokens,
      }

    default:
      return state
  }
}

function reducer(state: RunCheckpoint | null, action: CheckpointAction): RunCheckpoint | null {
  switch (action.type) {
    case 'SET_CHECKPOINT':
      return action.payload
    case 'APPLY_EVENT':
      return state ? applyEvent(state, action.payload) : state
    default:
      return state
  }
}

export interface UseLiveRunDetailOptions {
  wsUrl?: string
  live?: boolean
}

export function useLiveRunDetail(runId: string, options: UseLiveRunDetailOptions = {}) {
  const { wsUrl, live = true } = options

  const query = useQuery({
    queryKey: queryKeys.runs.detail(runId),
    queryFn: () => api.runs.get(runId),
    enabled: !!runId,
    refetchInterval: (q) => {
      if (!live) return false
      return q.state.data?.status === 'running' ? 10000 : false
    },
  })

  const [checkpoint, dispatch] = useReducer(reducer, null)

  useEffect(() => {
    if (query.data) {
      dispatch({ type: 'SET_CHECKPOINT', payload: query.data })
    }
  }, [query.data])

  const { events, status: wsStatus } = useLiveEvents(runId, {
    wsUrl,
    enabled: live && checkpoint?.status === 'running',
  })

  const lastApplied = useMemo(() => events.length, [events.length])

  useEffect(() => {
    for (let i = lastApplied; i < events.length; i++) {
      dispatch({ type: 'APPLY_EVENT', payload: events[i] })
    }
  }, [events, lastApplied])

  const refresh = useCallback(() => {
    query.refetch()
  }, [query])

  return {
    ...query,
    data: checkpoint ?? query.data,
    events,
    wsStatus,
    refresh,
  }
}
