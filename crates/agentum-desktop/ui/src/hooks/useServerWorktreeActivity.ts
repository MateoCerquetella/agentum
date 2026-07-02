import { useEffect } from 'react'
import { useAppStore } from '@/store'
import { listSessions } from '@/runtime/agentum-server-client'
import { subscribeServerEvents, type ServerEventFrame } from '@/runtime/server-events-bus'
import { installWindowVisibilityInterval } from '@/lib/window-visibility-interval'
import { reconnectBackoffMs as backoffMs } from '@/runtime/reconnect-backoff'
import {
  buildWorktreeActivitySnapshot,
  indexSessionsByWorktree,
  serverWorktreeActivityFromEvent,
  type ServerWorktreeLiveActivity,
  type WorktreeLike
} from '@/lib/server-worktree-activity-map'

// Backstop poll: lifecycle events (session.started / session.crashed) keep the
// alive-set fresh in real time, but a periodic refetch self-heals any missed
// event (e.g. a session killed outside agentum). Loopback call — cheap.
const REFRESH_INTERVAL_MS = 30_000

function collectWorktrees(): WorktreeLike[] {
  const byRepo = useAppStore.getState().worktreesByRepo
  const out: WorktreeLike[] = []
  for (const list of Object.values(byRepo)) {
    for (const wt of list) {
      // Skip a worktree without a resolved path (degraded/failed detection,
      // unreachable remote host, partial hydration). Passing an undefined path
      // into the join would otherwise throw and blank the whole overlay.
      if (wt.path) {
        out.push({ id: wt.id, path: wt.path })
      }
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
    // Separate fast-retry track for the very first snapshot after (re)mount —
    // e.g. a cold app relaunch where the embedded server isn't answering yet.
    let bootstrapTimer: ReturnType<typeof setTimeout> | null = null
    let bootstrapAttempt = 0

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

      // Fold every backing session's verdict into one entry per worktree,
      // keeping the MOST active (awaiting > working > idle) so a sibling
      // session's idle can't mask a working agent.
      const snapshot = buildWorktreeActivitySnapshot(
        aliveWorktreeIds,
        sessionToWorktree,
        activityBySessionId
      )
      useAppStore.getState().setServerWorktreeActivitySnapshot(snapshot)
    }

    const doRefresh = async (): Promise<boolean> => {
      let sessions: Awaited<ReturnType<typeof listSessions>>
      try {
        sessions = await listSessions()
      } catch {
        // Server not ready yet (or transient) — keep the last snapshot; the
        // bootstrap fast-retry and the interval heartbeat will retry.
        return false
      }
      if (disposed) {
        return false
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
      return true
    }

    // Single-flight: at mount this fires from three tracks nearly at once
    // (bootstrap fast-retry, the visibility interval's immediate run, the
    // events-bus onOpen) — share one listSessions round trip instead of
    // issuing three identical fetches at the moment the embedded server is
    // slowest to answer.
    let inFlightRefresh: Promise<boolean> | null = null
    const refresh = (): Promise<boolean> => {
      if (!inFlightRefresh) {
        inFlightRefresh = doRefresh().finally(() => {
          inFlightRefresh = null
        })
      }
      return inFlightRefresh
    }

    // Fast bootstrap so the sidebar shows the ACTUAL status right after an app
    // relaunch. The 30s heartbeat alone is too slow: on a cold start the
    // embedded server may not answer /api/sessions the instant this hook mounts,
    // and agents that were ALREADY running emit no `session.started` to trigger
    // an earlier refetch — so the map (and the /api/events replay routed through
    // it) would stay empty, leaving every dot idle, until the first heartbeat.
    // Retry with capped backoff until the FIRST successful snapshot; the
    // heartbeat + live events take over after that.
    const bootstrapRefresh = async (): Promise<void> => {
      if (disposed) {
        return
      }
      const ok = await refresh()
      if (ok || disposed) {
        return
      }
      bootstrapAttempt += 1
      bootstrapTimer = setTimeout(() => void bootstrapRefresh(), backoffMs(bootstrapAttempt))
    }

    const handleEvent = (ev: ServerEventFrame): void => {
      // A session coming up or dying changes the alive-set; refetch to learn it.
      if (ev.kind === 'session.started' || ev.kind === 'session.crashed') {
        void refresh()
        return
      }
      const verdict = serverWorktreeActivityFromEvent(ev)
      if (!verdict) {
        return
      }
      // Watchdog verdicts repeat (heartbeat re-affirms 'working' every tick).
      // An unchanged verdict can't change the folded snapshot, so skip the
      // remap: with many agents those redundant frames made remap() — an
      // O(worktrees + sessions) rebuild — the dominant per-event cost, even
      // though the setState behind it deduped.
      if (activityBySessionId.get(verdict.sessionId) === verdict.activity) {
        return
      }
      activityBySessionId.set(verdict.sessionId, verdict.activity)
      // Recompute so the worktree reflects the MOST-active of its sessions
      // (awaiting > working > idle, folded in buildWorktreeActivitySnapshot). A
      // per-session patch here let a sibling's idle/finished verdict clobber a
      // working agent — the "working shows idle" bug. remap() is pure + cheap
      // (no IO) and setServerWorktreeActivitySnapshot no-ops when unchanged; a
      // verdict for a not-yet-fetched session stays cached in activityBySessionId
      // and is applied on the next refresh (same as before).
      remap()
    }

    // Shared `/api/events` socket (server-events-bus): one connection + one
    // JSON.parse per frame for the whole renderer instead of a dedicated
    // socket here. onOpen fires on every (re)connect — the fresh stream
    // replays each session's current agent state, so refetch the session list
    // then to route the replay and self-heal the map after a reconnect.
    const unsubscribeEvents = subscribeServerEvents({
      onEvent: handleEvent,
      onOpen: () => void refresh()
    })

    void bootstrapRefresh()
    // Visibility-gated like the git-status/PR/ports polls: a hidden window
    // can't present the refreshed dots, so don't burn the loopback round trip;
    // the helper refreshes once immediately on re-show to catch up.
    const stopRefreshHeartbeat = installWindowVisibilityInterval({
      run: () => void refresh(),
      intervalMs: REFRESH_INTERVAL_MS
    })
    // Re-map when the worktree set changes (worktrees load async after this hook
    // mounts; a session fetched before its worktree existed must still bind).
    const unsubscribe = useAppStore.subscribe((state, prev) => {
      if (state.worktreesByRepo !== prev.worktreesByRepo) {
        remap()
      }
    })

    return () => {
      disposed = true
      if (bootstrapTimer) {
        clearTimeout(bootstrapTimer)
      }
      stopRefreshHeartbeat()
      unsubscribe()
      unsubscribeEvents()
      useAppStore.getState().clearServerWorktreeActivity()
    }
  }, [])
}
