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

/** A parsed `/api/events` frame. Shapes vary by `kind`; consumers narrow. */
export type ServerEventFrame = { kind?: unknown; session_id?: unknown; payload?: unknown }

export type ServerEventsSubscriber = {
  /** Called with every parsed frame (already deduplicated parse). */
  onEvent: (ev: ServerEventFrame) => void
  /**
   * Called when the shared socket (re)opens — and immediately at subscribe
   * time if it is already open. The server replays each session's current
   * agent state on every fresh connect; a subscriber that joins an
   * already-open socket has MISSED that replay, so any consumer needing a
   * coherent snapshot must self-heal here (refetch), not rely on the burst.
   */
  onOpen?: () => void
}

const subscribers = new Set<ServerEventsSubscriber>()
let ws: WebSocket | null = null
let socketOpen = false
let starting = false
let attempt = 0
let reconnectTimer: ReturnType<typeof setTimeout> | null = null

const backoffMs = (n: number): number =>
  Math.min(5000, 250 * 2 ** Math.min(n - 1, 5)) + Math.floor(Math.random() * 250)

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
      for (const sub of [...subscribers]) {
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
