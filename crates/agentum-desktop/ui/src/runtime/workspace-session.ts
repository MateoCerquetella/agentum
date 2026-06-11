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
  /** Server host id (from /api/hosts) to run the session on. Omitted/undefined
   *  runs on the local host. An SSH host runs the agent in tmux on the remote
   *  over SSH — the session is keyed by (workdir, tool, host) so a remote and a
   *  local workspace at the same path don't collide. */
  hostId?: string | null
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

/** A workspace session plus whether THIS call spawned its pane. `freshPane`
 *  is true only when start created a brand-new pane (a bare shell) — the only
 *  state where typing a launch command is safe. A reattach (the pane survived
 *  in tmux, possibly with an agent still running) reports false. */
export type WorkspaceSession = Session & { freshPane: boolean }

/**
 * Return a RUNNING server session for `(workdir, tool)`: reuse the existing one,
 * else create it, and start it (spawn its tmux pane) if it isn't already running
 * so the terminal stream has live output. Matching on workdir+tool keeps one
 * tmux pane per workspace surface instead of spawning a duplicate each reopen.
 */
export async function ensureWorkspaceSession(req: WorkspaceSessionRequest): Promise<WorkspaceSession> {
  const sessions = await listSessions()
  // Why: match on host too. The server reports a session's host as `hostId`
  // (absent/null = local), so a remote SSH session and a local one at the same
  // (workdir, tool) stay distinct instead of reattaching to the wrong pane.
  const wantHostId = req.hostId ?? null
  const existing = sessions.find(
    (s) => s.workdir === req.workdir && s.tool === req.tool && (s.host_id ?? null) === wantHostId
  )
  const session =
    existing ??
    (await createSession({
      name: req.name ?? sessionName(req.workdir, req.tool),
      workdir: req.workdir,
      tool: req.tool,
      worktree: req.worktree,
      host_id: req.hostId ?? undefined
    }))
  // Spawn the tmux pane if the session isn't live yet. Idempotent server-side;
  // a reattach to a live pane still succeeds (the server reports spawned=false).
  if (session.status !== 'running') {
    const started = await startSession(session.id).catch(() => null)
    if (started) {
      // The start response carries the TRUTHFUL post-start state (status,
      // tmux_target — the tab-bar tmux glyph depends on it), so no re-read is
      // needed. `spawned === true` is the server's word that the pane is a
      // fresh bare shell; anything else (reattach, or an older daemon that
      // doesn't report it) must be treated as possibly-live — fail safe by
      // never auto-typing into it.
      return { ...started, freshPane: started.spawned === true }
    }
    return { ...session, freshPane: false }
  }
  return { ...session, freshPane: false }
}
