import type { StateCreator } from 'zustand'
import type { AppState } from '../types'

/**
 * Spec 023 Part A (AC 1): marks a just-created workspace whose gated run the
 * engine actually OWNS (`gatedRunResultOwnsWorktree` → true at the composer
 * call site). Written ONLY by `maybeOfferWorkspaceHarnessRun`'s gated arm —
 * synchronously, before its first await, so the workspace never flashes the
 * bare picker — and cleared when the engine session becomes attachable, when
 * the run halts, or on the bounded no-run guard. No persist middleware:
 * nothing outlives the app session. Modeled on
 * slices/workspace-harness-offer.ts.
 */
export type GatedRunStarting = {
  worktreeId: string
  /** `worktree.path` at create time — the run match key
   *  (`HarnessStatus.workdir`, compared via `normalizeWorkdir`). */
  workdir: string
}

export type GatedRunStartingSlice = {
  gatedRunStartingByWorktreeId: Record<string, GatedRunStarting>
  setGatedRunStarting: (entry: GatedRunStarting) => void
  clearGatedRunStarting: (worktreeId: string) => void
}

export const createGatedRunStartingSlice: StateCreator<
  AppState,
  [],
  [],
  GatedRunStartingSlice
> = (set) => ({
  gatedRunStartingByWorktreeId: {},

  setGatedRunStarting: (entry) =>
    set((s) => ({
      gatedRunStartingByWorktreeId: {
        ...s.gatedRunStartingByWorktreeId,
        [entry.worktreeId]: entry
      }
    })),

  clearGatedRunStarting: (worktreeId) =>
    set((s) => {
      if (!(worktreeId in s.gatedRunStartingByWorktreeId)) {
        return s
      }
      const next = { ...s.gatedRunStartingByWorktreeId }
      delete next[worktreeId]
      return { gatedRunStartingByWorktreeId: next }
    })
})
