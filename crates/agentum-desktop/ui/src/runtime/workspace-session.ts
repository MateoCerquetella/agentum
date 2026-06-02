// Option A: map a desktop workspace (a worktree/repo the user opened) to a
// server session. Find-or-create so reopening a workspace reattaches to its
// existing tmux pane on the server instead of spawning a duplicate — the basis
// for git/fs/terminal flowing through /api/sessions/{id}/* for that workspace.
import {
  listSessions,
  createSession,
  startSession,
  type Session
} from './agentum-server-client'

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

/** Short stable token from a string (FNV-1a → base36), to disambiguate names. */
function shortHash(value: string): string {
  let h = 2166136261
  for (let i = 0; i < value.length; i += 1) {
    h ^= value.charCodeAt(i)
    h = Math.imul(h, 16777619)
  }
  return (h >>> 0).toString(36)
}

/**
 * Build a session name the server accepts (`validate_name`: ASCII
 * alphanumeric/-/_, ≤64). Non-conforming chars in the workdir basename/tool
 * collapse to `-`; a workdir hash suffix keeps two same-named folders distinct.
 */
function sessionName(workdir: string, tool: string): string {
  const clean = (s: string): string => s.replace(/[^a-zA-Z0-9_-]+/g, '-').replace(/^-+|-+$/g, '')
  const base = clean(basename(workdir)) || 'ws'
  const t = clean(tool) || 'terminal'
  return `${base}-${t}-${shortHash(workdir)}`.slice(0, 64).replace(/-+$/, '')
}

/**
 * Return a RUNNING server session for `(workdir, tool)`: reuse the existing one,
 * else create it, and start it (spawn its tmux pane) if it isn't already running
 * so the terminal stream has live output. Matching on workdir+tool keeps one
 * tmux pane per workspace surface instead of spawning a duplicate each reopen.
 */
export async function ensureWorkspaceSession(req: WorkspaceSessionRequest): Promise<Session> {
  const sessions = await listSessions()
  const existing = sessions.find((s) => s.workdir === req.workdir && s.tool === req.tool)
  const session =
    existing ??
    (await createSession({
      name: req.name ?? sessionName(req.workdir, req.tool),
      workdir: req.workdir,
      tool: req.tool,
      worktree: req.worktree
    }))
  // Spawn the tmux pane if the session isn't live yet. Idempotent server-side;
  // ignore "already running" so a reattach to a live pane still succeeds.
  if (session.status !== 'running') {
    await startSession(session.id).catch(() => {})
  }
  return session
}
