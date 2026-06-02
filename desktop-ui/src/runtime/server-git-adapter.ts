// Adapts the embedded server's git API to the desktop's GitStatusResult shape,
// so the source-control panel can run against a workspace's server session
// (Option A) instead of the local git preload. Flag-gated by the caller; the
// local path stays the default until verified in the running app.
import type {
  GitStatusResult,
  GitStatusEntry,
  GitUpstreamStatus as DesktopGitUpstreamStatus
} from '../../../shared/types'
import type { GitConflictOperation } from '../../../shared/git-status-types'
import { ensureWorkspaceSession } from './workspace-session'
import {
  gitStatusEntries,
  gitConflict,
  gitBranches,
  gitUpstream,
  gitStage,
  type GitStatusEntry as ServerStatusEntry,
  type GitConflictOp
} from './server-git-client'

function mapConflictOp(op: GitConflictOp): GitConflictOperation {
  // Server 'none' → desktop 'unknown' (its no-conflict sentinel).
  return op === 'none' ? 'unknown' : op
}

function mapEntry(e: ServerStatusEntry): GitStatusEntry {
  return {
    path: e.path,
    status: e.status,
    area: e.area,
    ...(e.oldPath ? { oldPath: e.oldPath } : {})
  }
}

function mapUpstream(u: {
  upstream: string | null
  ahead: number
  behind: number
}): DesktopGitUpstreamStatus {
  return {
    hasUpstream: u.upstream !== null,
    ...(u.upstream ? { upstreamName: u.upstream } : {}),
    ahead: u.ahead,
    behind: u.behind
  }
}

/**
 * Build a desktop `GitStatusResult` for `workdir` by fanning out across the
 * server session's git endpoints (status-entries + conflict + branch + upstream).
 * Upstream is best-effort — a worktree with no tracking branch still returns a
 * valid status rather than failing the whole panel.
 */
export async function getServerGitStatus(workdir: string): Promise<GitStatusResult> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  const [entries, conflict, branches] = await Promise.all([
    gitStatusEntries(session.id),
    gitConflict(session.id),
    gitBranches(session.id)
  ])
  const upstream = await gitUpstream(session.id).catch(() => null)
  return {
    entries: entries.map(mapEntry),
    conflictOperation: mapConflictOp(conflict.operation),
    ...(branches.current ? { branch: branches.current } : {}),
    ...(upstream ? { upstreamStatus: mapUpstream(upstream) } : {})
  }
}

/** Read the in-progress conflict op for a workspace's server session. */
export async function getServerGitConflictOperation(
  workdir: string
): Promise<GitConflictOperation> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  const { operation } = await gitConflict(session.id)
  return mapConflictOp(operation)
}

/** Read tracking-branch + ahead/behind for a workspace's server session. */
export async function getServerGitUpstreamStatus(
  workdir: string
): Promise<DesktopGitUpstreamStatus> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  return mapUpstream(await gitUpstream(session.id))
}

/**
 * Stage (`unstage=false`) or unstage (`true`) paths in a workspace's server
 * session. Staging is fully reversible, so this is safe to route ahead of the
 * destructive write ops (commit/discard), which await live verification.
 */
export async function serverGitStage(
  workdir: string,
  paths: string[],
  unstage: boolean
): Promise<void> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  await gitStage(session.id, paths, unstage)
}
