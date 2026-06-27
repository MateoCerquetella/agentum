// Pure mapping between server sessions / watchdog events and worktrees, used by
// the always-on "is this worktree's agent running?" sync (see
// hooks/useServerWorktreeActivity.ts). Kept IO-free so it's trivially testable.

/** The watchdog's activity verdict for a worktree's backing session, distilled
 *  from the `/api/events` stream. `awaiting` = blocked on the user. */
export type ServerWorktreeLiveActivity = 'working' | 'awaiting' | 'idle'

/** Minimal shape of a `GET /api/sessions` row we depend on. */
export type SessionLike = {
  id: string
  status: string
  workdir: string
  worktree_path?: string | null
}

/** Minimal worktree shape we match sessions against. */
export type WorktreeLike = {
  id: string
  path: string
}

export type SessionWorktreeIndex = {
  /** Worktree ids with at least one backing session the server reports as
   *  `running` (tmux pane alive on the host) — the baseline "running" signal. */
  aliveWorktreeIds: string[]
  /** session id → worktree id, so live `/api/events` activity can be routed to
   *  the right worktree without a pane being mounted. */
  sessionToWorktree: Map<string, string>
}

// Why: paths from the server (`session.workdir`) and the store
// (`worktree.path`) should be the same absolute path, but a trailing slash on
// one side would silently break the join. Normalize both ends the same way.
function normalizePath(path: string): string {
  const trimmed = path.trim()
  if (trimmed.length > 1 && trimmed.endsWith('/')) {
    return trimmed.replace(/\/+$/, '')
  }
  return trimmed
}

/**
 * Join server sessions to worktrees by filesystem path. A session's effective
 * worktree path is `worktree_path` when the server provisioned a dedicated
 * checkout (board card-start), otherwise its `workdir` (the agent runs in the
 * worktree). Returns the alive worktree set plus the session→worktree index.
 */
export function indexSessionsByWorktree(
  sessions: readonly SessionLike[],
  worktrees: readonly WorktreeLike[]
): SessionWorktreeIndex {
  const pathToWorktreeId = new Map<string, string>()
  for (const wt of worktrees) {
    pathToWorktreeId.set(normalizePath(wt.path), wt.id)
  }

  const aliveWorktreeIds = new Set<string>()
  const sessionToWorktree = new Map<string, string>()
  for (const session of sessions) {
    const effectivePath = normalizePath(session.worktree_path ?? session.workdir)
    const worktreeId = pathToWorktreeId.get(effectivePath)
    if (!worktreeId) {
      continue
    }
    sessionToWorktree.set(session.id, worktreeId)
    if (session.status === 'running') {
      aliveWorktreeIds.add(worktreeId)
    }
  }

  return { aliveWorktreeIds: [...aliveWorktreeIds], sessionToWorktree }
}

/**
 * Distill a raw `/api/events` frame into a (sessionId, activity) verdict, or
 * null for events we don't track. Mirrors the watchdog transitions:
 * `agent.working` → working, `agent.awaiting_input` → awaiting (needs you),
 * `agent.finished` → idle, `agent.input_resolved` → the resolved state.
 */
export function serverWorktreeActivityFromEvent(ev: {
  kind?: unknown
  session_id?: unknown
  payload?: unknown
}): { sessionId: string; activity: ServerWorktreeLiveActivity } | null {
  const sessionId = typeof ev.session_id === 'string' ? ev.session_id : null
  if (!sessionId) {
    return null
  }
  switch (ev.kind) {
    case 'agent.working':
      return { sessionId, activity: 'working' }
    case 'agent.awaiting_input':
      return { sessionId, activity: 'awaiting' }
    case 'agent.finished':
      return { sessionId, activity: 'idle' }
    case 'agent.input_resolved': {
      const state =
        ev.payload && typeof ev.payload === 'object'
          ? (ev.payload as { state?: unknown }).state
          : undefined
      return { sessionId, activity: state === 'working' ? 'working' : 'idle' }
    }
    default:
      return null
  }
}
