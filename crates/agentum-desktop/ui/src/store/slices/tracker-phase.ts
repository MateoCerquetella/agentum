import type { StateCreator } from 'zustand'
import type { AppState } from '../types'
import type { TrackerLiveOverlay, TrackerPhaseWire } from '@/lib/tracker-phase'

/**
 * Spec 014 F2: the live tracker overlay per worktree, fed by
 * `useTrackerPhaseSync` from `tracker.*` events on the shared `/api/events`
 * bus. Events are HINTS layered over the persisted `trackerPhase` on the
 * worktree row (the lossy bus is never the only source of truth) — see
 * `deriveTrackerChip`. Modeled on slices/server-worktree-activity.ts
 * (no-op-on-equal writes so redundant frames don't re-render every card).
 */
export type TrackerPhaseSlice = {
  trackerLiveByWorktreeId: Record<string, TrackerLiveOverlay>
  /** A `tracker.phase_changed` for this worktree: the phase moved, and any
   *  pipeline write also clears `status/blocked` server-side — so attention
   *  clears here in lockstep (AC 11). */
  patchTrackerPhase: (worktreeId: string, phase: TrackerPhaseWire) => void
  /** A `tracker.blocked` for this worktree: flag the needs-attention variant,
   *  keeping whatever live phase was already known. */
  setTrackerAttention: (worktreeId: string) => void
  /** Drop all live overlays (endpoint switch / teardown). */
  clearTrackerLive: () => void
}

export const createTrackerPhaseSlice: StateCreator<AppState, [], [], TrackerPhaseSlice> = (
  set
) => ({
  trackerLiveByWorktreeId: {},

  patchTrackerPhase: (worktreeId, phase) =>
    set((s) => {
      const existing = s.trackerLiveByWorktreeId[worktreeId]
      if (existing && existing.phase === phase && !existing.attention) {
        return s
      }
      return {
        trackerLiveByWorktreeId: {
          ...s.trackerLiveByWorktreeId,
          [worktreeId]: { phase, attention: false }
        }
      }
    }),

  setTrackerAttention: (worktreeId) =>
    set((s) => {
      const existing = s.trackerLiveByWorktreeId[worktreeId]
      if (existing?.attention) {
        return s
      }
      return {
        trackerLiveByWorktreeId: {
          ...s.trackerLiveByWorktreeId,
          [worktreeId]: { ...(existing ?? {}), attention: true }
        }
      }
    }),

  clearTrackerLive: () =>
    set((s) => {
      if (Object.keys(s.trackerLiveByWorktreeId).length === 0) {
        return s
      }
      return { trackerLiveByWorktreeId: {} }
    })
})
