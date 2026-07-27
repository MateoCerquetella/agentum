// Worktree registry / git-worktree ops over the embedded agentum-server
// (`/api/worktrees/*`). The logic moved off the desktop's native commands into
// `agentum-server/src/routes/worktrees.rs`; this is the typed boundary the UI
// calls. Worktree ids contain `/`, so id-bearing ops are POST-with-body.
import { getJson, postJson, qs } from './server-http'
import type { TrackerPhaseWire } from '@/lib/tracker-phase'

export type WorktreeTrackerReconcileResult = {
  reconciled: boolean
  phase: TrackerPhaseWire | null
}

export type WorktreeTrackerTransitionResult = {
  applied: true
  phase: TrackerPhaseWire
}

export function worktreesDetected(repoId: string): Promise<unknown> {
  return getJson(`/api/worktrees/detected${qs({ repoId })}`)
}

export function worktreesLineage(): Promise<unknown> {
  return getJson('/api/worktrees/lineage')
}

export function worktreesResolvePrBase(): Promise<unknown> {
  return getJson('/api/worktrees/resolve-pr-base')
}

export function worktreesUpdateMeta(
  worktreeId: string,
  updates: Record<string, unknown>
): Promise<unknown> {
  return postJson('/api/worktrees/update-meta', { worktreeId, updates })
}

export function worktreesReconcileGithubStatus(
  worktreeId: string,
  statusOptionId: string
): Promise<WorktreeTrackerReconcileResult> {
  return postJson<WorktreeTrackerReconcileResult>('/api/worktrees/reconcile-github-status', {
    worktreeId,
    statusOptionId
  })
}

export function worktreesReconcileLinearStatus(
  worktreeId: string,
  stateName: string
): Promise<WorktreeTrackerReconcileResult> {
  return postJson<WorktreeTrackerReconcileResult>('/api/worktrees/reconcile-linear-status', {
    worktreeId,
    stateName
  })
}

export function worktreesTransitionTracker(
  worktreeId: string,
  targetPhase: TrackerPhaseWire
): Promise<WorktreeTrackerTransitionResult> {
  return postJson<WorktreeTrackerTransitionResult>('/api/worktrees/transition-tracker', {
    worktreeId,
    targetPhase
  })
}

export function worktreesCreate(args: {
  repoId: string
  name: string
  baseBranch?: string
  branchNameOverride?: string
  displayName?: string
  // Linked work-item metadata (spec 004 AC 2). `linkedPR` is the UI's wire
  // casing (shared/types.ts); the server aliases it onto its camelCase field.
  linkedIssue?: number
  linkedPR?: number
  linkedLinearIssue?: string
  // Spec 021: the per-project tracker bind (github -> url, linear -> url). The
  // server's CreateBody accepts these; the remote RPC path already sent them.
  trackerProvider?: string
  trackerUrl?: string
}): Promise<unknown> {
  return postJson('/api/worktrees/create', args)
}

export function worktreesRemove(args: {
  worktreeId: string
  force?: boolean
  skipArchive?: boolean
}): Promise<unknown> {
  return postJson('/api/worktrees/remove', args)
}

export function worktreesPersistSortOrder(orderedIds: string[]): Promise<unknown> {
  return postJson('/api/worktrees/sort-order', { orderedIds })
}

export function worktreesForceDeleteBranch(args: {
  worktreeId: string
  branchName: string
  expectedHead?: string
}): Promise<unknown> {
  return postJson('/api/worktrees/force-delete-branch', args)
}
