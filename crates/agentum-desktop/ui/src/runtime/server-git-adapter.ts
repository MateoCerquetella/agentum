// Adapts the embedded server's git API to the desktop's GitStatusResult shape,
// so the source-control panel can run against a workspace's server session
// (Option A) instead of the local git preload. Flag-gated by the caller; the
// local path stays the default until verified in the running app.
import type {
  GitStatusResult,
  GitStatusEntry,
  GitBranchCompareResult,
  GitCommitCompareResult,
  GitDiffResult,
  GitUpstreamStatus as DesktopGitUpstreamStatus
} from '@/shared/types'
import type { GitConflictOperation } from '@/shared/git-status-types'
import type { GitHistoryOptions, GitHistoryResult } from '@/shared/git-history'
import { ensureWorkspaceSession } from './workspace-session'
import {
  gitStatusEntries,
  gitConflict,
  gitBranches,
  gitUpstream,
  gitStage,
  gitBranchCompare,
  gitCommitCompare,
  gitFetch,
  gitPull,
  gitCommitStaged,
  gitDiscard,
  gitPush,
  gitRebase,
  gitAbortMerge,
  gitAbortRebase,
  gitFile,
  gitCheckIgnore,
  gitFastForward,
  gitRemoteFileUrl,
  gitBlob,
  gitHistory,
  type GitBlob,
  type GitStatusEntry as ServerStatusEntry,
  type GitConflictOp,
  type GitFileRevision
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

/** Compare the workspace branch against `baseRef` (3-dot). The server response
 *  is structurally the desktop's GitBranchCompareResult. */
export async function getServerGitBranchCompare(
  workdir: string,
  baseRef: string
): Promise<GitBranchCompareResult> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  return gitBranchCompare(session.id, baseRef)
}

/** Diff a single commit against its parent in the workspace's server session. */
export async function getServerGitCommitCompare(
  workdir: string,
  commitId: string
): Promise<GitCommitCompareResult> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  return gitCommitCompare(session.id, commitId)
}

/** `git fetch --all --prune` in the workspace's server session (non-destructive). */
export async function serverGitFetch(workdir: string): Promise<void> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  await gitFetch(session.id)
}

/** Fast-forward-only pull in the workspace's server session (won't lose work). */
export async function serverGitPull(workdir: string): Promise<void> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  await gitPull(session.id)
}

// --- Write ops with side effects. Correct, well-defined mappings to a single
// git command each; behind the same opt-in flag (default off). ---

/** Commit the staged index with `message`. Mirrors the desktop's commit action;
 *  returns `{success}` (errors surfaced, not thrown, like the local path). */
export async function serverGitCommit(
  workdir: string,
  message: string
): Promise<{ success: boolean; error?: string }> {
  try {
    const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
    await gitCommitStaged(session.id, message)
    return { success: true }
  } catch (error) {
    return { success: false, error: String(error) }
  }
}

/** Discard tracked paths (restore to HEAD). DESTRUCTIVE — loses uncommitted edits. */
export async function serverGitDiscard(workdir: string, paths: string[]): Promise<void> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  await gitDiscard(session.id, paths)
}

/** Push the current branch (sets upstream on first push). */
export async function serverGitPush(workdir: string): Promise<void> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  await gitPush(session.id)
}

/** Rebase the worktree branch onto `baseRef`. */
export async function serverGitRebase(workdir: string, baseRef: string): Promise<void> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  await gitRebase(session.id, baseRef)
}

/** `git merge --abort` (recovery). */
export async function serverGitAbortMerge(workdir: string): Promise<void> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  await gitAbortMerge(session.id)
}

/** `git rebase --abort` (recovery). */
export async function serverGitAbortRebase(workdir: string): Promise<void> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  await gitAbortRebase(session.id)
}

/**
 * Build the desktop's side-by-side diff for a file by fetching two revisions
 * from the workspace's server session. The desktop diff view compares content,
 * not unified text, so map the {staged, compareAgainstHead} flags to revisions:
 *   - compareAgainstHead → HEAD vs worktree (the combined view)
 *   - staged             → HEAD vs index
 *   - else (unstaged)    → index vs worktree
 * Returned as a text result; binary content arrives as lossy UTF-8 (a unified
 * binary-aware diff is a follow-up if the renderer needs it).
 */
export async function getServerGitDiff(
  workdir: string,
  args: { filePath: string; staged: boolean; compareAgainstHead?: boolean }
): Promise<GitDiffResult> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  let original: GitFileRevision
  let modified: GitFileRevision
  if (args.compareAgainstHead) {
    original = 'head'
    modified = 'worktree'
  } else if (args.staged) {
    original = 'head'
    modified = 'index'
  } else {
    original = 'index'
    modified = 'worktree'
  }
  const [orig, mod] = await Promise.all([
    gitFile(session.id, args.filePath, original),
    gitFile(session.id, args.filePath, modified)
  ])
  return {
    kind: 'text',
    originalContent: orig.content,
    modifiedContent: mod.content,
    originalIsBinary: false,
    modifiedIsBinary: false
  }
}

/** The empty side of a content-pair (e.g. a root commit's parent, or an add). */
const EMPTY_BLOB: GitBlob = { content: '', isBinary: false, truncated: false }

/** MIME for binary preview, mirroring the desktop's native `guess_mime`. */
function guessMime(filePath: string): string | undefined {
  const lower = filePath.toLowerCase()
  if (lower.endsWith('.png')) return 'image/png'
  if (lower.endsWith('.jpg') || lower.endsWith('.jpeg')) return 'image/jpeg'
  if (lower.endsWith('.gif')) return 'image/gif'
  if (lower.endsWith('.webp')) return 'image/webp'
  if (lower.endsWith('.svg')) return 'image/svg+xml'
  if (lower.endsWith('.pdf')) return 'application/pdf'
  return undefined
}

/** Decode a base64 blob to a UTF-8 string (atob yields latin1 bytes). */
function base64ToUtf8(b64: string): string {
  const bin = atob(b64)
  const bytes = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i += 1) {
    bytes[i] = bin.charCodeAt(i)
  }
  return new TextDecoder().decode(bytes)
}

/**
 * Build the desktop's content-pair `GitDiffResult` from two server blobs,
 * binary-aware so image/PDF diffs preview from base64 — matching the native
 * `diff_result_value`. Text blobs are decoded from base64 to UTF-8.
 */
function diffResultFromBlobs(
  filePath: string,
  original: GitBlob,
  modified: GitBlob
): GitDiffResult {
  if (original.isBinary || modified.isBinary) {
    const mime = guessMime(filePath)
    return {
      kind: 'binary',
      originalContent: original.content,
      modifiedContent: modified.content,
      originalIsBinary: original.isBinary,
      modifiedIsBinary: modified.isBinary,
      ...(mime ? { isImage: mime.startsWith('image/'), mimeType: mime } : {})
    } as GitDiffResult
  }
  return {
    kind: 'text',
    originalContent: base64ToUtf8(original.content),
    modifiedContent: base64ToUtf8(modified.content),
    originalIsBinary: false,
    modifiedIsBinary: false
  }
}

/** Subset of `paths` git ignores, via the workspace's server session. */
export async function getServerGitCheckIgnored(
  workdir: string,
  paths: string[]
): Promise<string[]> {
  if (paths.length === 0) {
    return []
  }
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  return gitCheckIgnore(session.id, paths)
}

/** `git merge --ff-only @{upstream}` in the workspace's server session. */
export async function serverGitFastForward(workdir: string): Promise<void> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  await gitFastForward(session.id)
}

/** Web URL for a file/line on origin's host (null when unavailable). */
export async function getServerGitRemoteFileUrl(
  workdir: string,
  relativePath: string,
  line: number
): Promise<string | null> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  const { url } = await gitRemoteFileUrl(session.id, relativePath, line)
  return url
}

/**
 * Content-pair diff for a single commit (commit vs its parent), mirroring the
 * native `git_commit_diff`. No parent (root commit) → the original side is empty.
 */
export async function getServerGitCommitDiff(
  workdir: string,
  args: { commitOid: string; parentOid?: string | null; filePath: string; oldPath?: string }
): Promise<GitDiffResult> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  const old = args.oldPath ?? args.filePath
  const [original, modified] = await Promise.all([
    args.parentOid ? gitBlob(session.id, old, args.parentOid) : Promise.resolve(EMPTY_BLOB),
    gitBlob(session.id, args.filePath, args.commitOid)
  ])
  return diffResultFromBlobs(args.filePath, original, modified)
}

/**
 * Content-pair diff for a branch comparison (base vs HEAD), mirroring the native
 * `git_branch_diff`: base = mergeBase || baseOid, modified = headOid.
 */
export async function getServerGitBranchDiff(
  workdir: string,
  args: {
    compare: { baseRef: string; baseOid: string; headOid: string; mergeBase: string }
    filePath: string
    oldPath?: string
  }
): Promise<GitDiffResult> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  const base = args.compare.mergeBase || args.compare.baseOid
  const old = args.oldPath ?? args.filePath
  const [original, modified] = await Promise.all([
    base ? gitBlob(session.id, old, base) : Promise.resolve(EMPTY_BLOB),
    args.compare.headOid
      ? gitBlob(session.id, args.filePath, args.compare.headOid)
      : Promise.resolve(EMPTY_BLOB)
  ])
  return diffResultFromBlobs(args.filePath, original, modified)
}

/**
 * Commit history for the workspace's server session. The server returns the same
 * (HEAD-scoped) shape the desktop's native `git_history` produced, so the cast
 * preserves the existing local behaviour exactly.
 */
export async function getServerGitHistory(
  workdir: string,
  options: GitHistoryOptions = {}
): Promise<GitHistoryResult> {
  const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
  return gitHistory(session.id, options.limit) as unknown as GitHistoryResult
}
