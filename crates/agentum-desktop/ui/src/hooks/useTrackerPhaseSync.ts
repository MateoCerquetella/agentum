import { useEffect } from 'react'
import { useAppStore } from '@/store'
import { subscribeServerEvents, type ServerEventFrame } from '@/runtime/server-events-bus'
import {
  matchEventToWorktree,
  trackerEventFromFrame,
  type TrackerWorktreeRow
} from '@/lib/tracker-phase'

function collectTrackerRows(): TrackerWorktreeRow[] {
  const byRepo = useAppStore.getState().worktreesByRepo
  const out: TrackerWorktreeRow[] = []
  for (const list of Object.values(byRepo)) {
    for (const wt of list) {
      out.push({ id: wt.id, trackerUrl: wt.trackerUrl ?? null })
    }
  }
  return out
}

/**
 * Spec 014 F2: route `tracker.phase_changed` / `tracker.blocked` events from
 * the shared `/api/events` bus (server-events-bus — no extra socket) into the
 * tracker-phase slice, joined to worktrees by id (or trackerUrl for
 * automation/MCP emitters that carry `worktree_id: null`). Unmatched events are
 * dropped — the chip then waits for the persisted-phase re-fetch (the events
 * are hints, never the only truth). Mount once at the app root, beside
 * useServerWorktreeActivity.
 */
export function useTrackerPhaseSync(): void {
  useEffect(() => {
    const handleEvent = (ev: ServerEventFrame): void => {
      const evt = trackerEventFromFrame(ev)
      if (!evt) {
        return
      }
      // Read the worktree set fresh per event (tracker events are rare —
      // transitions, not heartbeats — so a per-event scan is cheap).
      const worktreeId = matchEventToWorktree(evt, collectTrackerRows())
      if (!worktreeId) {
        return
      }
      const store = useAppStore.getState()
      if (evt.kind === 'phase') {
        store.patchTrackerPhase(worktreeId, evt.phase)
      } else {
        store.setTrackerAttention(worktreeId)
      }
    }

    const unsubscribe = subscribeServerEvents({ onEvent: handleEvent })
    return () => {
      unsubscribe()
      useAppStore.getState().clearTrackerLive()
    }
  }, [])
}
