// Worktree registry / git-worktree ops over the embedded agentum-server
// (`/api/worktrees/*`). The logic moved off the desktop's native commands into
// `agentum-server/src/routes/worktrees.rs`; this is the typed boundary the UI
// calls. Worktree ids contain `/`, so id-bearing ops are POST-with-body.
import { getJson, postJson, qs } from './server-http'

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

export function worktreesCreate(args: {
  repoId: string
  name: string
  baseBranch?: string
  branchNameOverride?: string
  displayName?: string
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
