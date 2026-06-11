// Byte-flow → working/idle detection for server-session agent panes whose
// titles carry no activity signal (OpenCode, Codex, …). The daemon watchdog
// already classifies these by polling the pane body for a busy spinner; the
// desktop sees the SAME pane bytes live over the stream WS, so we derive the
// same answer here without a second round trip: bytes arriving = the pane is
// redrawing = working; a sustained quiet window = the turn ended = idle.
//
// Title-signaling agents (Claude/Cursor/Gemini) never use this — their caller
// gates it off the moment a title reports working/permission, so the precise
// title path stays authoritative for them.

export type PaneActivityState = 'idle' | 'working'

export type PaneActivityTrackerOptions = {
  /** Quiet period after the last byte before the pane is treated as idle.
   *  Mirrors the watchdog's IDLE_AFTER_QUIET so both surfaces agree. */
  idleAfterMs: number
  /** Fired on the idle → working edge (first activity after a quiet pane). */
  onWorking: () => void
  /** Fired when the pane has produced no bytes for `idleAfterMs`. */
  onIdle: () => void
  /** Injectable timer hooks so tests can drive the clock deterministically.
   *  Default to the global timer functions. */
  setTimer?: (fn: () => void, ms: number) => ReturnType<typeof setTimeout>
  clearTimer?: (handle: ReturnType<typeof setTimeout>) => void
}

export type PaneActivityTracker = {
  /** Record that the pane emitted output right now. */
  noteActivity: () => void
  /** Current derived state. */
  state: () => PaneActivityState
  /** Cancel the idle timer; no further callbacks fire. */
  dispose: () => void
}

export function createPaneActivityTracker(
  opts: PaneActivityTrackerOptions
): PaneActivityTracker {
  const setTimer = opts.setTimer ?? ((fn, ms) => setTimeout(fn, ms))
  const clearTimer = opts.clearTimer ?? ((handle) => clearTimeout(handle))

  let state: PaneActivityState = 'idle'
  let timer: ReturnType<typeof setTimeout> | null = null
  let disposed = false

  const armIdleTimer = (): void => {
    if (timer !== null) {
      clearTimer(timer)
    }
    timer = setTimer(() => {
      timer = null
      if (disposed || state === 'idle') {
        return
      }
      state = 'idle'
      opts.onIdle()
    }, opts.idleAfterMs)
  }

  return {
    noteActivity: () => {
      if (disposed) {
        return
      }
      if (state !== 'working') {
        state = 'working'
        opts.onWorking()
      }
      armIdleTimer()
    },
    state: () => state,
    dispose: () => {
      disposed = true
      if (timer !== null) {
        clearTimer(timer)
        timer = null
      }
    }
  }
}
