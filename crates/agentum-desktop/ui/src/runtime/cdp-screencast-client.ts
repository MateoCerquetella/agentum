// Embedded-server transport for the in-agentum CDP browser screencast (009c-3).
//
// Renders the agent-driven headless CDP-Chromium INSIDE agentum's pane by
// streaming the server's `WS /api/cdp-browser/screencast` bridge: binary `0x62`
// frames flow in (the same protocol `RemoteBrowserPagePane` already decodes),
// pane input/navigation flows back out as `browser.*` JSON. This replaces the
// stubbed native `api.runtimeEnvironments.subscribe('browser.screencast')` path
// (see crates/agentum-desktop/src/commands/runtime.rs) with the thin-shell route
// straight to the embedded server — one transport for local AND host (the only
// difference is the `cdpPort`, which for a host is the 009a `ssh -L` tunnel port
// on `127.0.0.1`).
import { apiUrl, getServerEndpoint, wsUrl } from './server-endpoint'

export type CdpScreencastFormat = 'jpeg' | 'png'

/** Screencast knobs, forwarded to the server as query params. All optional. */
export type CdpScreencastOptions = {
  /** CDP port to attach to. Omit for the shared local browser (server default).
   *  For an SSH host, this is the tunneled `127.0.0.1` port (009a). */
  cdpPort?: number
  format?: CdpScreencastFormat
  quality?: number
  maxWidth?: number
  maxHeight?: number
  /** CDP frame throttle. Default 1 (every frame) — `2` drops the only frame a
   *  static page emits, leaving the pane blank until a repaint. */
  everyNthFrame?: number
  /** Per-worktree isolation: attach to (and launch on demand) THIS worktree's own
   *  browser instead of the shared one, so worktrees don't share tabs. Ignored if
   *  `cdpPort` is set (explicit/tunneled browser). */
  worktreeId?: string
}

/** Stream callbacks. Names mirror the legacy `subscribe()` callbacks so the pane
 *  rewire is a near-drop-in: a `ready` control → `onReady`, each binary frame →
 *  `onBinary(bytes)` (decode with `decodeBrowserScreencastFrame`), a server
 *  `error` → `onError`, and a terminal close (gave up reconnecting, or `end`) →
 *  `onClose`. */
export type CdpScreencastHandlers = {
  onReady?: (info: { format: CdpScreencastFormat }) => void
  onBinary: (bytes: Uint8Array) => void
  onError?: (message: string) => void
  onClose?: () => void
  /** Fired when a transient drop triggers a reconnect attempt (1-based). */
  onReconnecting?: (attempt: number) => void
}

/** Handle to a live screencast: push input, or tear it down. */
export type CdpScreencastSubscription = {
  /** Send one `browser.*` interaction to the browser. `method` is e.g.
   *  `browser.mouseMove`; `params` carries `x/y/button/dx/dy/key/url` — the
   *  server reads only the fields it needs and ignores the rest, so the pane's
   *  existing `{worktree,page,...}` params pass through unchanged. */
  sendInput: (method: string, params?: Record<string, unknown>) => void
  /** Tear down: stops reconnecting and closes the socket without firing onClose. */
  close: () => void
}

const RECONNECT_BASE_MS = 500
const RECONNECT_MAX_MS = 5_000
/** Give up (fire onClose) after this many consecutive failed reconnects — the
 *  browser singleton may be gone (stopped/crashed); endless retries would spin. */
const MAX_RECONNECT_ATTEMPTS = 5

function buildQuery(opts: CdpScreencastOptions, token: string | null): string {
  const p = new URLSearchParams()
  if (token) {
    p.set('token', token)
  }
  if (opts.cdpPort != null) {
    p.set('cdpPort', String(opts.cdpPort))
  }
  if (opts.format) {
    p.set('format', opts.format)
  }
  if (opts.quality != null) {
    p.set('quality', String(opts.quality))
  }
  if (opts.maxWidth != null) {
    p.set('maxWidth', String(opts.maxWidth))
  }
  if (opts.maxHeight != null) {
    p.set('maxHeight', String(opts.maxHeight))
  }
  if (opts.everyNthFrame != null) {
    p.set('everyNthFrame', String(opts.everyNthFrame))
  }
  if (opts.worktreeId) {
    p.set('worktreeId', opts.worktreeId)
  }
  const qs = p.toString()
  return qs ? `?${qs}` : ''
}

/** Open a screencast stream to the embedded server's CDP bridge. Resolves once
 *  the WS URL is built and the first socket is constructed (not on first frame —
 *  the caller flips to the pane on `onReady`). Reconnects transient drops with
 *  capped backoff; gives up (onClose) after `MAX_RECONNECT_ATTEMPTS`. */
export async function openCdpScreencast(
  opts: CdpScreencastOptions,
  handlers: CdpScreencastHandlers
): Promise<CdpScreencastSubscription> {
  const { token } = await getServerEndpoint()
  const base = await wsUrl('/api/cdp-browser/screencast')
  const url = `${base}${buildQuery(opts, token)}`

  let ws: WebSocket | null = null
  let disposed = false
  let attempt = 0
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null

  const clearReconnect = (): void => {
    if (reconnectTimer != null) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
  }

  const scheduleReconnect = (): void => {
    if (disposed) {
      return
    }
    attempt += 1
    if (attempt > MAX_RECONNECT_ATTEMPTS) {
      handlers.onClose?.()
      return
    }
    handlers.onReconnecting?.(attempt)
    const delay = Math.min(RECONNECT_BASE_MS * 2 ** (attempt - 1), RECONNECT_MAX_MS)
    reconnectTimer = setTimeout(connect, delay)
  }

  function connect(): void {
    if (disposed) {
      return
    }
    const sock = new WebSocket(url)
    sock.binaryType = 'arraybuffer'
    ws = sock

    sock.addEventListener('message', (event: MessageEvent) => {
      if (sock !== ws) {
        return // superseded
      }
      if (event.data instanceof ArrayBuffer) {
        handlers.onBinary(new Uint8Array(event.data))
        return
      }
      // Text frames are control JSON: ready / error / end.
      if (typeof event.data === 'string') {
        handleControl(event.data)
      }
    })

    sock.addEventListener('open', () => {
      if (sock !== ws) {
        return
      }
      // A successful open clears the backoff: the next drop starts fresh, so a
      // long-running stream isn't permanently capped by early flaps.
      attempt = 0
    })

    sock.addEventListener('close', () => {
      if (sock !== ws || disposed) {
        return // we superseded or tore down — not a drop to recover from
      }
      scheduleReconnect()
    })

    // `error` precedes `close`; let `close` drive reconnect so we don't double-fire.
    sock.addEventListener('error', () => {})
  }

  function handleControl(text: string): void {
    let msg: { type?: string; format?: string; message?: string }
    try {
      msg = JSON.parse(text) as typeof msg
    } catch {
      return
    }
    switch (msg.type) {
      case 'ready':
        handlers.onReady?.({ format: msg.format === 'png' ? 'png' : 'jpeg' })
        break
      case 'error':
        handlers.onError?.(msg.message ?? 'screencast error')
        break
      case 'end':
        // A clean server-side end (browser closed) — terminal, no reconnect.
        disposed = true
        clearReconnect()
        handlers.onClose?.()
        break
      default:
        break
    }
  }

  connect()

  return {
    sendInput: (method, params) => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ method, params: params ?? {} }))
      }
      // Dropped while reconnecting: input is transient (a moved mouse, a key);
      // silently discarding beats queueing stale interactions for replay.
    },
    close: () => {
      disposed = true
      clearReconnect()
      const sock = ws
      ws = null
      sock?.close()
    }
  }
}

/** An element clip in CSS-viewport px, as the server's `node_at_point` returns it. */
export type CdpNodeClip = { x: number; y: number; width: number; height: number; scale: number }

/** Result of {@link cdpNodeAtPoint}: the resolved element (clip + label, and — when
 *  `capture` was set — a sharp PNG path + base64), or a miss (`no_node`/`no_box`). */
export type CdpNodeAtPointResult =
  | {
      ok: true
      label: string
      clip: CdpNodeClip
      /** Present only when called with `capture`. */
      path?: string
      image_b64?: string
      image_width?: number
      image_height?: number
      bytes?: number
    }
  | { ok: false; code: string }

/**
 * Hit-test the shared CDP page at a viewport pixel `(x, y)` (CSS px, from
 * {@link pointToDevice}) for the in-pane annotate picker. `capture` also returns a
 * sharp element PNG (path + base64) for the comment-card thumbnail. Routes to the
 * SAME persistent Chromium the screencast renders, so coordinates line up. Never
 * throws — a transport/HTTP failure resolves to `{ ok:false, code }`.
 */
export async function cdpNodeAtPoint(
  x: number,
  y: number,
  capture: boolean,
  opts?: { worktreeId?: string; cdpPort?: number }
): Promise<CdpNodeAtPointResult> {
  try {
    const { token } = await getServerEndpoint()
    const url = await apiUrl('/api/cdp-browser/node-at-point')
    const res = await fetch(url, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        ...(token ? { authorization: `Bearer ${token}` } : {})
      },
      body: JSON.stringify({
        x,
        y,
        capture,
        // worktreeId routes the hit-test to the SAME per-worktree browser the
        // screencast renders (server resolves it to that worktree's cdpPort).
        ...(opts?.worktreeId ? { worktreeId: opts.worktreeId } : {}),
        ...(opts?.cdpPort != null ? { cdpPort: opts.cdpPort } : {})
      })
    })
    if (!res.ok) {
      return { ok: false, code: `http_${res.status}` }
    }
    return (await res.json()) as CdpNodeAtPointResult
  } catch {
    return { ok: false, code: 'transport' }
  }
}
