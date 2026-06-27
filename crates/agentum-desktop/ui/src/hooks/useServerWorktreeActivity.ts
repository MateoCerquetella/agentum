import { useEffect } from 'react'
import { useAppStore } from '@/store'
import { listSessions } from '@/runtime/agentum-server-client'
import { getServerEndpoint, wsUrl } from '@/runtime/server-endpoint'
import type { ServerWorktreeActivityEntry } from '@/store/slices/server-worktree-activity'
import {
  indexSessionsByWorktree,
  serverWorktreeActivityFromEvent,
  type ServerWorktreeLiveActivity,
  type WorktreeLike
} from '@/lib/server-worktree-activity-map'

// Backstop poll: lifecycle events (session.started / session.crashed) keep the
// alive-set fresh in real time, but a periodic refetch self-heals any missed
// event (e.g. a session killed outside agentum). Loopback call — cheap.
const REFRESH_INTERVAL_MS = 30_000

const backoffMs = (n: number): number =>
  Math.min(5000, 250 * 2 ** Math.min(n - 1, 5)) + Math.floor(Math.random() * 250)

function collectWorktrees(): WorktreeLike[] {
  const byRepo = useAppStore.getState().worktreesByRepo
  const out: WorktreeLike[] = []
  for (const list of Object.values(byRepo)) {
    for (const wt of list) {
      out.push({ id: wt.id, path: wt.path })
    }
  }
  return out
}

/**
 * Keep the sidebar's per-worktree "is this agent running?" state in sync with
 * the SERVER, independent of whether any terminal pane is mounted.
 *
 * Why this exists: after an app relaunch the renderer-local status heuristics
 * (OSC title + live PTY) start cold, and the per-pane watchdog subscription
 * (server-session-activity.ts) only runs once a pane mounts — so the sidebar
 * shows every running agent as idle until you open it. This hook fixes that with
 * two app-wide, always-on sources:
 *   - `GET /api/sessions` → which worktrees have a live (`running`) backing tmux
 *     session (the "alive = running" baseline).
 *   - `WS /api/events` watchdog stream → the live working / needs-attention
 *     overlay, routed to worktrees via the session→worktree index.
 *
 * Mount once at the app root. See resolveWorktreeStatus for how the dot consumes
 * `isAlive` / `liveActivity`.
 */
export function useServerWorktreeActivity(): void {
  useEffect(() => {
    let disposed = false
    let ws: WebSocket | null = null
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null
    let refreshTimer: ReturnType<typeof setInterval> | null = null
    let attempt = 0

    // session id → worktree id, rebuilt from the latest /api/sessions snapshot.
    const sessionToWorktree = new Map<string, string>()
    // Last watchdog verdict per RUNNING session. Pruned on each refresh so a
    // stopped session's stale verdict can't keep a worktree falsely "alive".
    const activityBySessionId = new Map<string, ServerWorktreeLiveActivity>()
    let lastSessions: Awaited<ReturnType<typeof listSessions>> = []

    // Recompute the per-worktree snapshot from the cached sessions + activity and
    // the CURRENT worktree set (read fresh so a late worktree load still maps).
    const remap = (): void => {
      const { aliveWorktreeIds, sessionToWorktree: index } = indexSessionsByWorktree(
        lastSessions,
        collectWorktrees()
      )
      sessionToWorktree.clear()
      for (const [sessionId, worktreeId] of index) {
        sessionToWorktree.set(sessionId, worktreeId)
      }

      const snapshot: Record<string, ServerWorktreeActivityEntry> = {}
      for (const worktreeId of aliveWorktreeIds) {
        snapshot[worktreeId] = { alive: true }
      }
      for (const [sessionId, activity] of activityBySessionId) {
        const worktreeId = sessionToWorktree.get(sessionId)
        if (!worktreeId) {
          continue
        }
        // The cache only holds running sessions (see prune in refresh), so the
        // worktree is in the alive set — overlay its activity.
        snapshot[worktreeId] = { alive: true, activity }
      }
      useAppStore.getState().setServerWorktreeActivitySnapshot(snapshot)
    }

    const refresh = async (): Promise<void> => {
      let sessions: Awaited<ReturnType<typeof listSessions>>
      try {
        sessions = await listSessions()
      } catch {
        // Server not ready yet (or transient) — keep the last snapshot; the
        // interval and lifecycle events will retry.
        return
      }
      if (disposed) {
        return
      }
      lastSessions = sessions
      // Drop cached activity for sessions that are gone or no longer running so a
      // stale 'working'/'idle' verdict can't resurrect an alive entry in remap.
      const runningIds = new Set(
        sessions.filter((s) => s.status === 'running').map((s) => s.id)
      )
      for (const sessionId of [...activityBySessionId.keys()]) {
        if (!runningIds.has(sessionId)) {
          activityBySessionId.delete(sessionId)
        }
      }
      remap()
    }

    const handleEvent = (raw: string): void => {
      let ev: { kind?: unknown; session_id?: unknown; payload?: unknown }
      try {
        ev = JSON.parse(raw)
      } catch {
        return
      }
      // A session coming up or dying changes the alive-set; refetch to learn it.
      if (ev.kind === 'session.started' || ev.kind === 'session.crashed') {
        void refresh()
        return
      }
      const verdict = serverWorktreeActivityFromEvent(ev)
      if (!verdict) {
        return
      }
      activityBySessionId.set(verdict.sessionId, verdict.activity)
      const worktreeId = sessionToWorktree.get(verdict.sessionId)
      // Known mapping → update now. Unknown (session not fetched yet) → leave it
      // cached; the next refresh / session.started applies it. We deliberately do
      // NOT refetch per unknown event, to avoid a refresh storm.
      if (worktreeId) {
        useAppStore.getState().patchServerWorktreeActivity(worktreeId, verdict.activity)
      }
    }

    const connect = async (): Promise<void> => {
      if (disposed) {
        return
      }
      let url: string
      try {
        const { token } = await getServerEndpoint()
        const base = await wsUrl('/api/events')
        url = token ? `${base}?token=${encodeURIComponent(token)}` : base
      } catch {
        attempt += 1
        reconnectTimer = setTimeout(() => void connect(), backoffMs(attempt))
        return
      }
      if (disposed) {
        return
      }
      const sock = new WebSocket(url)
      ws = sock
      sock.addEventListener('open', () => {
        attempt = 0
      })
      sock.addEventListener('message', (event) => {
        if (typeof event.data === 'string') {
          handleEvent(event.data)
        }
      })
      sock.addEventListener('close', () => {
        if (sock !== ws || disposed) {
          return
        }
        ws = null
        attempt += 1
        reconnectTimer = setTimeout(() => void connect(), backoffMs(attempt))
      })
      // 'error' is always followed by 'close', which drives reconnect.
    }

    void refresh()
    void connect()
    refreshTimer = setInterval(() => void refresh(), REFRESH_INTERVAL_MS)
    // Re-map when the worktree set changes (worktrees load async after this hook
    // mounts; a session fetched before its worktree existed must still bind).
    const unsubscribe = useAppStore.subscribe((state, prev) => {
      if (state.worktreesByRepo !== prev.worktreesByRepo) {
        remap()
      }
    })

    return () => {
      disposed = true
      if (reconnectTimer) {
        clearTimeout(reconnectTimer)
      }
      if (refreshTimer) {
        clearInterval(refreshTimer)
      }
      unsubscribe()
      ws?.close()
      useAppStore.getState().clearServerWorktreeActivity()
    }
  }, [])
}
