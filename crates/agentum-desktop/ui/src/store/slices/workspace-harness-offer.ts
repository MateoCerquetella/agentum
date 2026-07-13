import type { StateCreator } from 'zustand'
import type { AppState } from '../types'
import type { WorkspaceHarnessOffer } from '@/lib/workspace-harness-detect'

/**
 * Spec 015: the resolved "Start Harness run?" offer per just-created worktree.
 * Written ONLY by `maybeOfferWorkspaceHarnessRun` (a positive detection after
 * a workspace creation — D2), cleared by accept/dismiss/stale-purge. Holds the
 * RESOLVED offer, never a pending flag, so `HarnessSpecBanner` stays dumb
 * (select → render → accept/dismiss). No persist middleware: nothing outlives
 * the app session. Modeled on slices/tracker-phase.ts.
 */
export type WorkspaceHarnessOfferSlice = {
  harnessOfferByWorktreeId: Record<string, WorkspaceHarnessOffer>
  setWorkspaceHarnessOffer: (offer: WorkspaceHarnessOffer) => void
  clearWorkspaceHarnessOffer: (worktreeId: string) => void
}

export const createWorkspaceHarnessOfferSlice: StateCreator<
  AppState,
  [],
  [],
  WorkspaceHarnessOfferSlice
> = (set) => ({
  harnessOfferByWorktreeId: {},

  setWorkspaceHarnessOffer: (offer) =>
    set((s) => ({
      harnessOfferByWorktreeId: {
        ...s.harnessOfferByWorktreeId,
        [offer.worktreeId]: offer
      }
    })),

  clearWorkspaceHarnessOffer: (worktreeId) =>
    set((s) => {
      if (!(worktreeId in s.harnessOfferByWorktreeId)) {
        return s
      }
      const next = { ...s.harnessOfferByWorktreeId }
      delete next[worktreeId]
      return { harnessOfferByWorktreeId: next }
    })
})
