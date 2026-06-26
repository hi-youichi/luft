import { create } from 'zustand'

interface UIState {
  agentCardDensity: 'compact' | 'comfortable'
  eventStreamPaused: boolean
  selectedPhaseId: number | null
  runDialogOpen: boolean

  toggleDensity: () => void
  toggleEventPause: () => void
  setSelectedPhase: (id: number | null) => void
  setRunDialogOpen: (open: boolean) => void
}

export const useUIStore = create<UIState>((set) => ({
  agentCardDensity: 'compact',
  eventStreamPaused: false,
  selectedPhaseId: null,
  runDialogOpen: false,

  toggleDensity: () =>
    set((s) => ({
      agentCardDensity: s.agentCardDensity === 'compact' ? 'comfortable' : 'compact',
    })),
  toggleEventPause: () => set((s) => ({ eventStreamPaused: !s.eventStreamPaused })),
  setSelectedPhase: (id) => set({ selectedPhaseId: id }),
  setRunDialogOpen: (open) => set({ runDialogOpen: open }),
}))
