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
//
// Defensive against a missing path: a worktree row can lack a resolved `path`
// (degraded/failed detection, an unreachable remote host, or partial
// hydration), and a session can carry an empty workdir. Coerce to '' instead of
// throwing — one `undefined.trim()` here would blow up the whole `remap()` and
// silently blank EVERY sidebar dot, not just the offending worktree's.
function normalizePath(path: string | null | undefined): string {
  const trimmed = (path ?? '').trim()
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
    const key = normalizePath(wt.path)
    // Skip a path-less worktree rather than mapping every empty-path session to
    // it — and never let its missing path abort the whole index.
    if (!key) {
      continue
    }
    pathToWorktreeId.set(key, wt.id)
  }

  const aliveWorktreeIds = new Set<string>()
  const sessionToWorktree = new Map<string, string>()
  for (const session of sessions) {
    const effectivePath = normalizePath(session.worktree_path ?? session.workdir)
    if (!effectivePath) {
      continue
    }
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

/** One worktree's server-authoritative liveness + activity. Structurally equal
 *  to the store's `ServerWorktreeActivityEntry` (kept local to avoid a slice→map
 *  import cycle). */
export type WorktreeActivityEntry = { alive: boolean; activity?: ServerWorktreeLiveActivity }

/** Rank so a worktree with several backing sessions reflects its MOST active
 *  one. `awaiting` (blocked on the user) > `working` > `idle`, mirroring the dot
 *  priority in resolveWorktreeStatus. */
const ACTIVITY_PRIORITY: Record<ServerWorktreeLiveActivity, number> = {
  awaiting: 3,
  working: 2,
  idle: 1
}

/**
 * Fold per-session activity verdicts into one entry per worktree, keeping the
 * highest-priority activity across a worktree's sessions. This is the fix for
 * the "working agent shows idle" bug: a worktree can back MULTIPLE sessions
 * (an agent + a plain terminal, or two agent tabs), and a plain last-writer-wins
 * overlay let a sibling's `idle`/`finished` verdict clobber a live `working` /
 * `awaiting` one. Every alive worktree starts present with the `{alive:true}`
 * baseline; a session that has emitted any verdict also implies its worktree is
 * alive. Pure (no store/IO) so it's unit-tested directly.
 */
export function buildWorktreeActivitySnapshot(
  aliveWorktreeIds: readonly string[],
  sessionToWorktree: ReadonlyMap<string, string>,
  activityBySessionId: ReadonlyMap<string, ServerWorktreeLiveActivity>
): Record<string, WorktreeActivityEntry> {
  const snapshot: Record<string, WorktreeActivityEntry> = {}
  for (const worktreeId of aliveWorktreeIds) {
    snapshot[worktreeId] = { alive: true }
  }
  for (const [sessionId, activity] of activityBySessionId) {
    const worktreeId = sessionToWorktree.get(sessionId)
    if (!worktreeId) {
      continue
    }
    const current = snapshot[worktreeId]?.activity
    const winner =
      current && ACTIVITY_PRIORITY[current] >= ACTIVITY_PRIORITY[activity] ? current : activity
    snapshot[worktreeId] = { alive: true, activity: winner }
  }
  return snapshot
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
