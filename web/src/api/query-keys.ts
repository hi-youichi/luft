import type { RunFilters, RunLogsRequest } from './types'

export const queryKeys = {
  stats: ['stats'] as const,
  health: ['health'] as const,
  version: ['version'] as const,

  runs: {
    all: ['runs'] as const,
    list: (filters?: RunFilters) => ['runs', filters ?? null] as const,
    live: (filters?: RunFilters) => ['runs', 'live', filters ?? null] as const,
    detail: (runId: string) => ['runs', runId] as const,
    events: (runId: string) => ['runs', runId, 'events'] as const,
    logs: (runId: string, req?: RunLogsRequest) => ['runs', runId, 'logs', req ?? null] as const,
    artifacts: (runId: string) => ['runs', runId, 'artifacts'] as const,
  },

  workflows: {
    all: ['workflows'] as const,
    detail: (name: string) => ['workflows', name] as const,
  },

  backends: {
    all: ['backends'] as const,
    detail: (id: string) => ['backends', id] as const,
  },
} as const
