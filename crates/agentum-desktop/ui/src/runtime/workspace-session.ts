// Option A: map a desktop workspace (a worktree/repo the user opened) to a
// server session. Find-or-create so reopening a workspace reattaches to its
// existing tmux pane on the server instead of spawning a duplicate — the basis
// for git/fs/terminal flowing through /api/sessions/{id}/* for that workspace.
import { listSessions, createSession, type Session } from './agentum-server-client'

export type WorkspaceSessionRequest = {
  /** Absolute path of the worktree/repo the desktop opened. */
  workdir: string
  /** Tool to run in the pane (e.g. 'terminal', 'claude', 'codex'). */
  tool: string
  /** Stable display name; defaults to `<basename>:<tool>`. */
  name?: string
  /** Ask the server to create a dedicated git worktree for this session. */
  worktree?: boolean
}

function basename(path: string): string {
  const parts = path.replace(/[\\/]+$/, '').split(/[\\/]/)
  return parts[parts.length - 1] || path
}

/**
 * Return the existing server session for `(workdir, tool)` or create one.
 * Matching on workdir+tool keeps one tmux pane per workspace surface instead of
 * spawning a fresh pane every time the desktop reopens the same folder.
 */
export async function ensureWorkspaceSession(req: WorkspaceSessionRequest): Promise<Session> {
  const sessions = await listSessions()
  const existing = sessions.find((s) => s.workdir === req.workdir && s.tool === req.tool)
  if (existing) {
    return existing
  }
  return createSession({
    name: req.name ?? `${basename(req.workdir)}:${req.tool}`,
    workdir: req.workdir,
    tool: req.tool,
    worktree: req.worktree
  })
}
