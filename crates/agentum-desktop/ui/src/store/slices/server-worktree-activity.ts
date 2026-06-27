import type { StateCreator } from 'zustand'
import type { AppState } from '../types'
import type { ServerWorktreeLiveActivity } from '@/lib/server-worktree-activity-map'

export type ServerWorktreeActivityEntry = {
  /** A backing tmux session for this worktree is alive on the host (the server
   *  reports its status as `running`). This is the baseline "running" signal the
   *  sidebar shows after an app relaunch, even when no terminal pane is mounted —
   *  the renderer-local title/PTY heuristics are cold then. */
  alive: boolean
  /** Latest watchdog activity verdict for this worktree, once one has arrived on
   *  `/api/events`. Overlays the alive baseline: `working` / `awaiting` (needs
   *  you) / `idle`. Undefined until the first event. */
  activity?: ServerWorktreeLiveActivity
}

export type ServerWorktreeActivitySlice = {
  /** Server-authoritative per-worktree liveness + activity, independent of any
   *  mounted terminal pane. Populated by `useServerWorktreeActivity` from
   *  `GET /api/sessions` (alive) and the `WS /api/events` watchdog stream
   *  (activity). Read by `resolveWorktreeStatus` so the sidebar dot reflects the
   *  truth after relaunch instead of decaying to idle. */
  serverWorktreeActivityByWorktreeId: Record<string, ServerWorktreeActivityEntry>
  /** Replace the whole map atomically from a fresh `/api/sessions` snapshot. */
  setServerWorktreeActivitySnapshot: (next: Record<string, ServerWorktreeActivityEntry>) => void
  /** Overlay one worktree's live activity from a watchdog event. Any `agent.*`
   *  event implies the session is alive, so this also marks the worktree alive —
   *  keeping liveness fresh between session-snapshot refreshes. */
  patchServerWorktreeActivity: (worktreeId: string, activity: ServerWorktreeLiveActivity) => void
  /** Drop all server activity (e.g. endpoint switch / teardown). */
  clearServerWorktreeActivity: () => void
}

function snapshotsEqual(
  a: Record<string, ServerWorktreeActivityEntry>,
  b: Record<string, ServerWorktreeActivityEntry>
): boolean {
  const aKeys = Object.keys(a)
  if (aKeys.length !== Object.keys(b).length) {
    return false
  }
  for (const key of aKeys) {
    const ea = a[key]
    const eb = b[key]
    if (!eb || ea.alive !== eb.alive || ea.activity !== eb.activity) {
      return false
    }
  }
  return true
}

export const createServerWorktreeActivitySlice: StateCreator<
  AppState,
  [],
  [],
  ServerWorktreeActivitySlice
> = (set) => ({
  serverWorktreeActivityByWorktreeId: {},

  setServerWorktreeActivitySnapshot: (next) =>
    set((s) => {
      // Why: this runs on a refresh interval and on every session lifecycle
      // event. Bail when nothing actually changed so we don't bump the sort /
      // status epochs (and re-render every worktree card) on an idle heartbeat.
      if (snapshotsEqual(s.serverWorktreeActivityByWorktreeId, next)) {
        return s
      }
      return {
        serverWorktreeActivityByWorktreeId: next,
        // Why: bump both epochs in lockstep with the agent-status path — a
        // worktree's resolved status (hence its smart-sort class) can change
        // when its backing session goes alive/dead or starts/stops working.
        agentStatusEpoch: s.agentStatusEpoch + 1,
        sortEpoch: s.sortEpoch + 1
      }
    }),

  patchServerWorktreeActivity: (worktreeId, activity) =>
    set((s) => {
      const existing = s.serverWorktreeActivityByWorktreeId[worktreeId]
      if (existing && existing.alive && existing.activity === activity) {
        return s
      }
      return {
        serverWorktreeActivityByWorktreeId: {
          ...s.serverWorktreeActivityByWorktreeId,
          [worktreeId]: { alive: true, activity }
        },
        agentStatusEpoch: s.agentStatusEpoch + 1,
        sortEpoch: s.sortEpoch + 1
      }
    }),

  clearServerWorktreeActivity: () =>
    set((s) => {
      if (Object.keys(s.serverWorktreeActivityByWorktreeId).length === 0) {
        return s
      }
      return {
        serverWorktreeActivityByWorktreeId: {},
        agentStatusEpoch: s.agentStatusEpoch + 1,
        sortEpoch: s.sortEpoch + 1
      }
    })
})

export type ServerWorktreeActivitySelection = {
  isAlive: boolean
  liveActivity?: ServerWorktreeLiveActivity
}

// Stable reference for the (common) no-entry case so `useShallow` subscribers
// don't re-render when an unrelated worktree's activity changes.
const EMPTY_SELECTION: ServerWorktreeActivitySelection = { isAlive: false }

/** Per-worktree server liveness + activity, shaped for `resolveWorktreeStatus`.
 *  The map is typed optional so the selector tolerates partial store mocks in
 *  tests — the real store always initializes it to `{}`. */
export function selectServerWorktreeActivity(
  state: { serverWorktreeActivityByWorktreeId?: Record<string, ServerWorktreeActivityEntry> },
  worktreeId: string
): ServerWorktreeActivitySelection {
  const entry = state.serverWorktreeActivityByWorktreeId?.[worktreeId]
  if (!entry) {
    return EMPTY_SELECTION
  }
  return { isAlive: entry.alive, liveActivity: entry.activity }
}
