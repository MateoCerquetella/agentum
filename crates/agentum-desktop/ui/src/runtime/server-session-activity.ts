// Bridges the embedded server's authoritative agent-activity events
// (`WS /api/events`) into per-pane status handlers.
//
// Why this exists: the renderer derives a pane's working/idle/done state from
// its OSC title + byte flow, but it CANNOT tell "the agent paused to ask you a
// question" from "the agent finished its turn" — both look like a working→idle
// edge (Claude's title just goes idle when it shows a permission prompt). The
// server-side watchdog DOES know: it scans the pane for the prompt signatures
// and emits `agent.awaiting_input` / `agent.input_resolved`, plus
// `agent.working` / `agent.finished`. This module subscribes to that stream
// once for the whole app and fans each event out to the bound pane (keyed by
// server session id) so the sidebar dot reflects the truth.
//
// It also fixes the cold-load case: `/api/events` replays one current-state
// `agent.*` event per session on connect (`{"replay": true}`), and we cache the
// last state per session so a pane that mounts AFTER that replay still gets
// seeded — without it a running agent reads as idle until its next transition.
import { subscribeServerEvents } from './server-events-bus'

/** The pane-facing activity verdicts, normalized from the server's `agent.*`
 *  event kinds. */
export type ServerSessionActivityKind = 'awaiting_input' | 'input_resolved' | 'working' | 'finished'

export type ServerSessionActivityRecord = {
  kind: ServerSessionActivityKind
  /** Only meaningful for `input_resolved`: the state the agent resumed into
   *  (`'working'` | `'idle'` | `'unknown'`). */
  state?: string
}

export type ServerSessionActivityHandlers = {
  /** Agent is blocked on the user (permission prompt / multi-choice menu). */
  onAwaitingInput: () => void
  /** The block was answered/dismissed; `state` is the resumed activity. */
  onInputResolved: (state: string) => void
  /** Agent is actively working (used to seed the dot on reload). */
  onWorking: () => void
  /** Agent finished its turn (or was already finished at connect time). */
  onFinished: () => void
}

/** Map a raw `/api/events` event `kind` to a normalized activity record, or
 *  null when the event is not an agent-activity transition we care about. */
export function activityRecordFromEvent(ev: {
  kind?: unknown
  payload?: unknown
}): ServerSessionActivityRecord | null {
  switch (ev.kind) {
    case 'agent.awaiting_input':
      return { kind: 'awaiting_input' }
    case 'agent.input_resolved': {
      const state =
        ev.payload && typeof ev.payload === 'object'
          ? (ev.payload as { state?: unknown }).state
          : undefined
      return { kind: 'input_resolved', state: typeof state === 'string' ? state : 'unknown' }
    }
    case 'agent.working':
      return { kind: 'working' }
    case 'agent.finished':
      return { kind: 'finished' }
    default:
      return null
  }
}

/**
 * A registry that maps server session ids to per-pane activity handlers and
 * remembers each session's last-known activity. Pure (no IO) so it's unit
 * testable; the WS wiring below feeds it parsed events.
 */
export function createServerSessionActivityHub() {
  const handlersBySessionId = new Map<string, ServerSessionActivityHandlers>()
  const lastBySessionId = new Map<string, ServerSessionActivityRecord>()

  const dispatch = (
    handlers: ServerSessionActivityHandlers,
    record: ServerSessionActivityRecord
  ): void => {
    switch (record.kind) {
      case 'awaiting_input':
        handlers.onAwaitingInput()
        break
      case 'input_resolved':
        handlers.onInputResolved(record.state ?? 'unknown')
        break
      case 'working':
        handlers.onWorking()
        break
      case 'finished':
        handlers.onFinished()
        break
    }
  }

  return {
    /** Feed a parsed `/api/events` frame. Updates the per-session cache and
     *  notifies a registered handler. Ignores non-activity events. */
    handleEvent(ev: { kind?: unknown; session_id?: unknown; payload?: unknown }): void {
      const sessionId = typeof ev.session_id === 'string' ? ev.session_id : null
      if (!sessionId) {
        return
      }
      const record = activityRecordFromEvent(ev)
      if (!record) {
        return
      }
      lastBySessionId.set(sessionId, record)
      const handlers = handlersBySessionId.get(sessionId)
      if (handlers) {
        dispatch(handlers, record)
      }
    },

    /** Register a pane's handlers for `sessionId`. If a current state is already
     *  cached (the replay arrived before this pane mounted), it is delivered
     *  immediately. Returns an unregister fn. */
    register(sessionId: string, handlers: ServerSessionActivityHandlers): () => void {
      handlersBySessionId.set(sessionId, handlers)
      const last = lastBySessionId.get(sessionId)
      if (last) {
        dispatch(handlers, last)
      }
      return () => {
        // Guard against a remount race overwriting a newer registration.
        if (handlersBySessionId.get(sessionId) === handlers) {
          handlersBySessionId.delete(sessionId)
        }
      }
    },

    hasHandlers(): boolean {
      return handlersBySessionId.size > 0
    }
  }
}

export type ServerSessionActivityHub = ReturnType<typeof createServerSessionActivityHub>

// ── App-wide singleton + shared-bus wiring ──────────────────────────────────
const hub = createServerSessionActivityHub()

let busSubscribed = false

/**
 * Register a server-backed pane to receive its session's agent-activity
 * verdicts (awaiting-input / resolved / working / finished). The first pane
 * joins the hub onto the SHARED `/api/events` socket (server-events-bus) and
 * the subscription is kept for the app's lifetime — the hub's per-session
 * replay cache must not miss frames while panes churn. Returns an unregister
 * fn for the pane's dispose.
 */
export function registerServerSessionActivity(
  sessionId: string,
  handlers: ServerSessionActivityHandlers
): () => void {
  const unregister = hub.register(sessionId, handlers)
  if (!busSubscribed) {
    busSubscribed = true
    subscribeServerEvents({ onEvent: (ev) => hub.handleEvent(ev) })
  }
  return unregister
}
