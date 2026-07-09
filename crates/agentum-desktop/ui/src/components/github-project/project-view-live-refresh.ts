// Spec 014 F3: pure trailing-edge coalescer for the Projects-view live
// refresh. A burst of `tracker.*` events inside the window causes exactly ONE
// re-fetch. IO-free (no timers, no sockets) — the hook drives it with
// `setTimeout`; tests drive it with plain numbers.

/** How long the coalesce window stays open after the FIRST tracker event.
 *  A named constant, not user config (spec AC 7); also absorbs most of
 *  GitHub's read-after-write lag. */
export const PROJECT_VIEW_EVENT_REFETCH_COALESCE_MS = 2_000

/** Is this `/api/events` kind a tracker write we should re-fetch on? Both
 *  `tracker.phase_changed` and `tracker.blocked` move the Projects card. */
export function isTrackerEventKind(kind: unknown): boolean {
  return typeof kind === 'string' && kind.startsWith('tracker.')
}

export type CoalesceState = {
  /** When the open window started, or null when no window is open. */
  windowOpenedAtMs: number | null
}

export const initialCoalesceState: CoalesceState = { windowOpenedAtMs: null }

/**
 * An event arrived at `nowMs`. Trailing-edge: the FIRST event opens the window
 * and asks the caller to schedule ONE fire at `nowMs + windowMs`
 * (`schedule: true`); every further event inside the window mutates nothing.
 */
export function coalesceEvent(
  state: CoalesceState,
  nowMs: number
): { state: CoalesceState; schedule: boolean } {
  if (state.windowOpenedAtMs !== null) {
    return { state, schedule: false }
  }
  return { state: { windowOpenedAtMs: nowMs }, schedule: true }
}

/** The scheduled fire ran: close the window so the next event opens a new one.
 *  The caller invokes its refetch exactly when this is applied. */
export function coalesceFire(state: CoalesceState): CoalesceState {
  if (state.windowOpenedAtMs === null) {
    return state
  }
  return { windowOpenedAtMs: null }
}
