// Session-scoped git client over the embedded agentum-server
// (`/api/sessions/{id}/git/*`). Option A: a workspace's git operations run
// against its server session's worktree, the same surface the TUI/board use.
import { getJson, getText, postJson, qs } from './server-http'

/** `git status --porcelain` grouped. A file with staged + unstaged edits
 *  appears in both `staged` and `unstaged`. */
export type GitStatus = {
  staged: string[]
  unstaged: string[]
  untracked: string[]
}

export type GitFileRevision = 'head' | 'index' | 'worktree'

export type GitFile = {
  content: string
  /** True when the blob exceeded the server cap and was truncated. */
  truncated: boolean
}

export type GitCommitResult = { sha: string }

export function gitStatus(sessionId: string): Promise<GitStatus> {
  return getJson<GitStatus>(`/api/sessions/${sessionId}/git/status`)
}

export type GitStatusEntry = {
  path: string
  /** modified|added|deleted|renamed|untracked|copied. */
  status: GitChangeStatus | 'untracked'
  /** staged|unstaged|untracked. */
  area: 'staged' | 'unstaged' | 'untracked'
  oldPath?: string
}

/** Per-file working-tree changes (richer than gitStatus's path arrays). */
export function gitStatusEntries(sessionId: string): Promise<GitStatusEntry[]> {
  return getJson<GitStatusEntry[]>(`/api/sessions/${sessionId}/git/status-entries`)
}

/** Unified diff (text/plain). `staged` → `git diff --cached`, else worktree vs index. */
export function gitDiff(sessionId: string, path: string, staged = false): Promise<string> {
  return getText(`/api/sessions/${sessionId}/git/diff${qs({ path, staged })}`)
}

/** One revision of a file as UTF-8 text. Missing-at-revision returns empty content. */
export function gitFile(
  sessionId: string,
  path: string,
  rev: GitFileRevision = 'worktree'
): Promise<GitFile> {
  return getJson<GitFile>(`/api/sessions/${sessionId}/git/file${qs({ path, rev })}`)
}

/** Stage (`git add`) or, with `unstage`, `git restore --staged`. Returns fresh status. */
export function gitStage(
  sessionId: string,
  paths: string[],
  unstage = false
): Promise<GitStatus> {
  return postJson<GitStatus>(`/api/sessions/${sessionId}/git/stage`, { paths, unstage })
}

/** Commit the given paths with `message`; returns the new commit SHA. */
export function gitCommit(
  sessionId: string,
  message: string,
  paths: string[]
): Promise<GitCommitResult> {
  return postJson<GitCommitResult>(`/api/sessions/${sessionId}/git/commit`, { message, paths })
}

/** Commit whatever is staged in the index (no add); returns the new SHA. */
export function gitCommitStaged(sessionId: string, message: string): Promise<GitCommitResult> {
  return postJson<GitCommitResult>(`/api/sessions/${sessionId}/git/commit-staged`, { message })
}

export type GitBranches = {
  /** Current branch, or null in detached-HEAD. */
  current: string | null
  /** Local branch names (refs/heads). */
  branches: string[]
}

export type GitLogEntry = {
  sha: string
  subject: string
  author: string
  /** Author date, ISO-8601. */
  timestamp: string
}

/** Local branches + the current one. */
export function gitBranches(sessionId: string): Promise<GitBranches> {
  return getJson<GitBranches>(`/api/sessions/${sessionId}/git/branches`)
}

/** Recent commits (default 50, max 500). */
export function gitLog(sessionId: string, limit?: number): Promise<GitLogEntry[]> {
  return getJson<GitLogEntry[]>(`/api/sessions/${sessionId}/git/log${qs({ limit })}`)
}

/** `git fetch --all --prune`. */
export function gitFetch(sessionId: string): Promise<void> {
  return postJson<void>(`/api/sessions/${sessionId}/git/fetch`)
}

/** Fast-forward-only pull. */
export function gitPull(sessionId: string): Promise<void> {
  return postJson<void>(`/api/sessions/${sessionId}/git/pull`)
}

/** Push the current branch (sets upstream on first push). */
export function gitPush(sessionId: string): Promise<void> {
  return postJson<void>(`/api/sessions/${sessionId}/git/push`)
}

export type GitUpstreamStatus = {
  /** Upstream ref (e.g. 'origin/main'), or null when none is set. */
  upstream: string | null
  ahead: number
  behind: number
}

/** Restore tracked paths to HEAD (drops staged + worktree changes). */
export function gitDiscard(sessionId: string, paths: string[]): Promise<void> {
  return postJson<void>(`/api/sessions/${sessionId}/git/discard`, { paths })
}

/** Tracking branch + ahead/behind counts. */
export function gitUpstream(sessionId: string): Promise<GitUpstreamStatus> {
  return getJson<GitUpstreamStatus>(`/api/sessions/${sessionId}/git/upstream`)
}

export type GitConflictOp = 'merge' | 'rebase' | 'cherry-pick' | 'none'

/** Which conflict operation (if any) is mid-flight in the worktree. */
export function gitConflict(sessionId: string): Promise<{ operation: GitConflictOp }> {
  return getJson<{ operation: GitConflictOp }>(`/api/sessions/${sessionId}/git/conflict`)
}

/** `git rebase <baseRef>`. Rejects (with git's stderr) on conflict. */
export function gitRebase(sessionId: string, baseRef: string): Promise<void> {
  return postJson<void>(`/api/sessions/${sessionId}/git/rebase`, { base_ref: baseRef })
}

/** `git merge --abort`. */
export function gitAbortMerge(sessionId: string): Promise<void> {
  return postJson<void>(`/api/sessions/${sessionId}/git/abort-merge`)
}

/** `git rebase --abort`. */
export function gitAbortRebase(sessionId: string): Promise<void> {
  return postJson<void>(`/api/sessions/${sessionId}/git/abort-rebase`)
}

export type GitChangeStatus = 'modified' | 'added' | 'deleted' | 'renamed' | 'copied'

export type GitBranchChangeEntry = {
  path: string
  status: GitChangeStatus
  oldPath?: string
  added?: number
  removed?: number
}

export type GitBranchCompareSummary = {
  baseRef: string
  baseOid: string | null
  compareRef: string
  headOid: string | null
  mergeBase: string | null
  changedFiles: number
  commitsAhead?: number
  status: 'ready' | 'invalid-base' | 'unborn-head' | 'no-merge-base' | 'error'
}

export type GitBranchCompare = {
  summary: GitBranchCompareSummary
  entries: GitBranchChangeEntry[]
}

/** Diff the worktree's HEAD against `baseRef` (3-dot), with per-file counts. */
export function gitBranchCompare(sessionId: string, baseRef: string): Promise<GitBranchCompare> {
  return getJson<GitBranchCompare>(
    `/api/sessions/${sessionId}/git/branch-compare${qs({ base: baseRef })}`
  )
}

export type GitCommitCompareSummary = {
  commitOid: string
  parentOid: string | null
  compareRef: string
  baseRef: string
  changedFiles: number
  status: 'ready' | 'invalid-commit' | 'error'
}

export type GitCommitCompare = {
  summary: GitCommitCompareSummary
  entries: GitBranchChangeEntry[]
}

/** Diff a single commit against its first parent (root commit vs empty tree). */
export function gitCommitCompare(sessionId: string, commit: string): Promise<GitCommitCompare> {
  return getJson<GitCommitCompare>(
    `/api/sessions/${sessionId}/git/commit-compare${qs({ commit })}`
  )
}

/** Subset of `paths` that git ignores (`git check-ignore`). */
export function gitCheckIgnore(sessionId: string, paths: string[]): Promise<string[]> {
  return postJson<string[]>(`/api/sessions/${sessionId}/git/check-ignore`, { paths })
}

/** `git merge --ff-only @{upstream}` — fast-forward to the tracking branch. */
export function gitFastForward(sessionId: string): Promise<void> {
  return postJson<void>(`/api/sessions/${sessionId}/git/fast-forward`)
}

/** Web URL for a file/line on origin's host (null when there's no origin remote). */
export function gitRemoteFileUrl(
  sessionId: string,
  path: string,
  line: number
): Promise<{ url: string | null }> {
  return getJson<{ url: string | null }>(
    `/api/sessions/${sessionId}/git/remote-file-url${qs({ path, line })}`
  )
}

export type GitBlob = {
  /** Base64 of the file's bytes at the requested revision (empty if absent). */
  content: string
  /** True when the bytes contain a NUL — render as a binary/image preview. */
  isBinary: boolean
  truncated: boolean
}

/** One file's bytes at an arbitrary revision (`git show <commit>:<path>`), base64. */
export function gitBlob(sessionId: string, path: string, commit: string): Promise<GitBlob> {
  return getJson<GitBlob>(`/api/sessions/${sessionId}/git/blob${qs({ path, commit })}`)
}

export type GitHistoryEntry = {
  id: string
  parentIds: string[]
  subject: string
  message: string
  displayId: string
  author: string
  authorEmail: string
  timestamp: number | null
}

/** Mirrors the desktop's native `git_history` shape (HEAD-scoped commit list). */
export type GitHistory = {
  items: GitHistoryEntry[]
  hasIncomingChanges: boolean
  hasOutgoingChanges: boolean
  hasMore: boolean
  limit: number
  currentRef?: { id: string; name: string }
}

/** Recent commits + incoming/outgoing-vs-upstream flags + the current ref. */
export function gitHistory(sessionId: string, limit?: number): Promise<GitHistory> {
  return getJson<GitHistory>(`/api/sessions/${sessionId}/git/history${qs({ limit })}`)
}
