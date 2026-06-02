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
