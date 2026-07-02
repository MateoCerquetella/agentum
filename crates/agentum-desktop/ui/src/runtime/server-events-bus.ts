// ONE `/api/events` WebSocket for the whole renderer, fanned out to every
// consumer. Before this, three modules (the per-pane activity hub, the
// sidebar's worktree-activity sync, and the board's live refresh) each held
// their own socket to the same endpoint and each JSON.parse'd every frame —
// with many agents the bus is the chattiest stream in the app, so that
// tripled both the server's fan-out work and the renderer's parse work.
//
// The socket lives while at least one subscriber is registered and closes on
// the last unsubscribe; reconnects use the same capped-backoff policy the
// per-consumer sockets used. 'error' is always followed by 'close', which
// drives the reconnect.
import { getServerEndpoint, wsUrl } from './server-endpoint'
import { reconnectBackoffMs as backoffMs } from './reconnect-backoff'

/** A parsed `/api/events` frame. Shapes vary by `kind`; consumers narrow. */
export type ServerEventFrame = { kind?: unknown; session_id?: unknown; payload?: unknown }

export type ServerEventsSubscriber = {
  /** Called with every parsed frame (already deduplicated parse). */
  onEvent: (ev: ServerEventFrame) => void
  /**
   * Called when the shared socket (re)opens — and immediately at subscribe
   * time if it is already open. The server replays each session's current
   * agent state only on a fresh connect; the bus re-delivers its cached copy
   * of that per-session `agent.*` state to late subscribers (see
   * `lastAgentFrameBySession`), so agent activity needs no self-heal here —
   * but any OTHER snapshot a consumer depends on (session lists, board
   * state) must be refetched in this callback, not assumed from the burst.
   */
  onOpen?: () => void
}

const subscribers = new Set<ServerEventsSubscriber>()
// Latest `agent.*` frame per session, mirrored off the stream. The server's
// connect-time replay burst goes only to subscribers present at socket open;
// consumers that join later (a terminal pane mounting after the sidebar
// opened the socket) would otherwise never learn a running agent's current
// state until its next live transition. Delivered to every new subscriber at
// subscribe time; overwritten by each fresh replay/live frame.
const lastAgentFrameBySession = new Map<string, ServerEventFrame>()
let ws: WebSocket | null = null
let socketOpen = false
let starting = false
let attempt = 0
let reconnectTimer: ReturnType<typeof setTimeout> | null = null

const scheduleReconnect = (): void => {
  if (reconnectTimer || subscribers.size === 0) {
    return
  }
  attempt += 1
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null
    void ensureSocket()
  }, backoffMs(attempt))
}

async function ensureSocket(): Promise<void> {
  if (ws || starting || subscribers.size === 0) {
    return
  }
  starting = true
  try {
    const { token } = await getServerEndpoint()
    const base = await wsUrl('/api/events')
    const url = token ? `${base}?token=${encodeURIComponent(token)}` : base
    // Everyone may have unsubscribed while we awaited the endpoint.
    if (subscribers.size === 0) {
      return
    }
    const sock = new WebSocket(url)
    ws = sock
    sock.addEventListener('open', () => {
      if (sock !== ws) {
        return
      }
      attempt = 0
      socketOpen = true
      // Copy: an onOpen may subscribe/unsubscribe synchronously.
      for (const sub of [...subscribers]) {
        sub.onOpen?.()
      }
    })
    sock.addEventListener('message', (event) => {
      if (typeof event.data !== 'string') {
        return
      }
      let ev: ServerEventFrame
      try {
        ev = JSON.parse(event.data) as ServerEventFrame
      } catch {
        // Ignore malformed frames rather than tearing the stream down.
        return
      }
      if (
        typeof ev.kind === 'string' &&
        ev.kind.startsWith('agent.') &&
        typeof ev.session_id === 'string'
      ) {
        lastAgentFrameBySession.set(ev.session_id, ev)
      }
      // Iterate the live Set (no snapshot): onEvent handlers don't mutate the
      // subscriber set, and this runs per frame on the app's chattiest
      // stream — a [...spread] here was steady GC pressure for nothing.
      for (const sub of subscribers) {
        sub.onEvent(ev)
      }
    })
    sock.addEventListener('close', () => {
      if (sock !== ws) {
        return
      }
      ws = null
      socketOpen = false
      scheduleReconnect()
    })
  } catch {
    scheduleReconnect()
  } finally {
    starting = false
  }
}

/**
 * Register for the shared `/api/events` stream. Lazily opens the app-wide
 * socket; returns an unsubscribe fn. See [`ServerEventsSubscriber.onOpen`]
 * for the replay caveat when joining an already-open socket.
 */
export function subscribeServerEvents(subscriber: ServerEventsSubscriber): () => void {
  subscribers.add(subscriber)
  if (socketOpen) {
    subscriber.onOpen?.()
  } else {
    void ensureSocket()
  }
  // Re-deliver the latest known agent state (the replay burst this late
  // subscriber missed). After onOpen, mirroring the real stream's order:
  // connect, then replay frames.
  if (lastAgentFrameBySession.size > 0) {
    for (const frame of lastAgentFrameBySession.values()) {
      subscriber.onEvent(frame)
    }
  }
  return () => {
    if (!subscribers.delete(subscriber)) {
      return
    }
    if (subscribers.size > 0) {
      return
    }
    // Last consumer left — drop the socket instead of holding an idle
    // connection; the next subscriber reopens it and self-heals via onOpen.
    if (reconnectTimer) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    const sock = ws
    ws = null
    socketOpen = false
    attempt = 0
    sock?.close()
  }
}
