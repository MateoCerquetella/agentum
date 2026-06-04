// The worktree registry + git-worktree ops moved off native Tauri commands into
// the embedded agentum-server (`/api/worktrees/*`); this namespace now calls the
// server (server-worktree-client). No native command remains for worktrees.
import { subscribe } from './core'
import type { AgentumApi } from './contract'
import {
  worktreesCreate,
  worktreesRemove,
  worktreesUpdateMeta,
  worktreesDetected,
  worktreesPersistSortOrder,
  worktreesForceDeleteBranch,
  worktreesLineage,
  worktreesResolvePrBase
} from '../runtime/server-worktree-client'

export const worktrees = {
  create: (...args: any[]) =>
    worktreesCreate({
      repoId: args[0]?.repoId,
      name: args[0]?.name,
      baseBranch: args[0]?.baseBranch,
      branchNameOverride: args[0]?.branchNameOverride,
      displayName: args[0]?.displayName
    }),
  remove: (...args: any[]) =>
    worktreesRemove({
      worktreeId: args[0]?.worktreeId,
      force: args[0]?.force,
      skipArchive: args[0]?.skipArchive
    }),
  updateMeta: (...args: any[]) => worktreesUpdateMeta(args[0]?.worktreeId, args[0]?.updates ?? {}),
  listDetected: (...args: any[]) => worktreesDetected(args[0]?.repoId),
  persistSortOrder: (...args: any[]) => worktreesPersistSortOrder(args[0]?.orderedIds ?? []),
  forceDeletePreservedBranch: (...args: any[]) =>
    worktreesForceDeleteBranch({
      worktreeId: args[0]?.worktreeId,
      branchName: args[0]?.branchName,
      expectedHead: args[0]?.expectedHead
    }),
  listLineage: () => worktreesLineage(),
  resolvePrBase: () => worktreesResolvePrBase(),
  // Native returned None; lineage tracking isn't ported.
  updateLineage: () => Promise.resolve(null),
  // Tauri events were never emitted by the native commands (no-op today); kept for
  // API parity. Source-control surfaces refresh by re-fetching.
  onBaseStatus: (cb: (p: any) => void) => subscribe('worktrees-base-status', cb),
  onChanged: (cb: (p: any) => void) => subscribe('worktrees-changed', cb),
  onRemoteBranchConflict: (cb: (p: any) => void) => subscribe('worktrees-remote-branch-conflict', cb)
} satisfies AgentumApi['worktrees']
