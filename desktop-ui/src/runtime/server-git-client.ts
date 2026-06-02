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
