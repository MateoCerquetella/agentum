import { useEffect, useRef } from 'react'
import { subscribeServerEvents } from '@/runtime/server-events-bus'
import {
  PROJECT_VIEW_EVENT_REFETCH_COALESCE_MS,
  coalesceEvent,
  coalesceFire,
  initialCoalesceState,
  isTrackerEventKind
} from './project-view-live-refresh'

/**
 * Spec 014 F3: while the Projects view is mounted, a `tracker.*` event on the
 * shared `/api/events` socket triggers ONE debounced re-fetch of the active
 * view (the pure coalescer above; 2 s trailing-edge window). No interval, no
 * poll — the subscription lives only while mounted, and unmounting
 * unsubscribes, which is the "hidden/inactive views fetch nothing" guarantee
 * (ProjectViewWrapper mounts only when Projects mode is shown).
 */
export function useProjectViewLiveRefresh(refetch: () => void): void {
  // Latest-callback ref so the (single) subscription never re-subscribes when
  // the wrapper re-renders with a new closure.
  const refetchRef = useRef(refetch)
  refetchRef.current = refetch

  useEffect(() => {
    let state = initialCoalesceState
    let timer: ReturnType<typeof setTimeout> | null = null

    const unsubscribe = subscribeServerEvents({
      onEvent: (ev) => {
        if (!isTrackerEventKind(ev.kind)) {
          return
        }
        const next = coalesceEvent(state, Date.now())
        state = next.state
        if (next.schedule) {
          timer = setTimeout(() => {
            timer = null
            state = coalesceFire(state)
            refetchRef.current()
          }, PROJECT_VIEW_EVENT_REFETCH_COALESCE_MS)
        }
      }
    })

    return () => {
      unsubscribe()
      if (timer) {
        // A pending fire dies with the view — an unmounted board never fetches.
        clearTimeout(timer)
      }
    }
  }, [])
}
