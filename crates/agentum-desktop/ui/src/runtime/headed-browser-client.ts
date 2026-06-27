// Client for the "Open Browser (persistent)" surface: a real HEADED Chrome
// window the agent drives over CDP (see the headed-agent-browser design spec).
// Unlike the screencast client, there is no stream — Chrome paints its own OS
// window natively; this just asks the embedded server to launch/stop it and
// reports the CDP port the agentum MCP attaches to.
import { apiUrl, getServerEndpoint, wsUrl } from './server-endpoint'

export type HeadedBrowserStatus = {
  running: boolean
  /** CDP port the agentum MCP attaches to (only when running). */
  port?: number
  cdpEndpoint?: string
}

/**
 * Launch (or attach to) a worktree's persistent headed Chrome window. Idempotent:
 * a second call returns the already-running browser's port. Throws on a transport
 * or server error so the caller can surface an actionable toast.
 */
export async function launchHeadedBrowser(worktreeId: string): Promise<HeadedBrowserStatus> {
  const { token } = await getServerEndpoint()
  const url = await apiUrl('/api/cdp-browser/headed')
  const res = await fetch(url, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      ...(token ? { authorization: `Bearer ${token}` } : {})
    },
    body: JSON.stringify({ worktreeId })
  })
  if (!res.ok) {
    // The server puts the fail-loud message (e.g. the Chromium install hint) in
    // the body — surface it verbatim rather than a bare status code.
    const detail = await res.text().catch(() => '')
    throw new Error(detail.trim() || `headed browser launch failed (HTTP ${res.status})`)
  }
  return (await res.json()) as HeadedBrowserStatus
}

/** Stop a worktree's persistent headed Chrome window. Idempotent. */
export async function stopHeadedBrowser(worktreeId: string): Promise<void> {
  const { token } = await getServerEndpoint()
  const url = await apiUrl('/api/cdp-browser/headed')
  await fetch(url, {
    method: 'DELETE',
    headers: {
      'content-type': 'application/json',
      ...(token ? { authorization: `Bearer ${token}` } : {})
    },
    body: JSON.stringify({ worktreeId })
  })
}

/**
 * Arm the in-page annotate overlay in the worktree's persistent (headed) Chrome
 * window. The user then clicks an element in the real Chrome window; the overlay
 * beacons the annotation back to the server, which rebroadcasts it on `/api/events`
 * as `browser.annotation`. Requires a headed browser to already be open (throws with
 * the server's actionable message otherwise).
 */
export async function armHeadedAnnotate(worktreeId: string): Promise<void> {
  const { token } = await getServerEndpoint()
  const url = await apiUrl('/api/cdp-browser/annotate')
  const res = await fetch(url, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      ...(token ? { authorization: `Bearer ${token}` } : {})
    },
    body: JSON.stringify({ worktreeId })
  })
  if (!res.ok) {
    const detail = await res.text().catch(() => '')
    throw new Error(detail.trim() || `could not arm annotate (HTTP ${res.status})`)
  }
}

/** A `browser.annotation` event payload as beaconed by the headed-browser overlay
 *  (same shape as the WKWebView `agentumgrab://annotation/add` payload). */
export type HeadedAnnotation = {
  pageId?: string
  comment?: string
  intent?: string
  payload?: {
    page?: { url?: string; title?: string }
    target?: { tagName?: string; selector?: string; textSnippet?: string }
  }
}

export type AnnotationStream = { close: () => void }

/**
 * Subscribe to the global event bus (`WS /api/events`) and invoke `onAnnotation`
 * for every `browser.annotation` event — what the persistent-browser overlay
 * beaconed back. Auto-reconnects with capped backoff (mirrors `openBoardEventStream`).
 */
export async function openBrowserAnnotationStream(
  onAnnotation: (a: HeadedAnnotation) => void
): Promise<AnnotationStream> {
  const { token } = await getServerEndpoint()
  const base = await wsUrl('/api/events')
  const url = token ? `${base}?token=${encodeURIComponent(token)}` : base

  let ws: WebSocket | null = null
  let disposed = false
  let attempt = 0
  let timer: ReturnType<typeof setTimeout> | null = null

  const backoffMs = (n: number): number =>
    Math.min(5000, 250 * 2 ** Math.min(n - 1, 5)) + Math.floor(Math.random() * 250)

  const connect = (): void => {
    if (disposed) return
    const sock = new WebSocket(url)
    ws = sock
    sock.addEventListener('open', () => {
      attempt = 0
    })
    sock.addEventListener('message', (event) => {
      if (typeof event.data !== 'string') return
      try {
        const ev = JSON.parse(event.data) as { kind?: string; payload?: HeadedAnnotation }
        if (ev.kind === 'browser.annotation' && ev.payload) {
          onAnnotation(ev.payload)
        }
      } catch {
        // Ignore malformed frames rather than tearing the stream down.
      }
    })
    sock.addEventListener('close', () => {
      if (sock !== ws || disposed) return
      attempt += 1
      timer = setTimeout(connect, backoffMs(attempt))
    })
  }

  connect()

  return {
    close: () => {
      disposed = true
      if (timer) clearTimeout(timer)
      ws?.close()
    }
  }
}

/** Render a received headed-browser annotation as a concise agent-ready prompt
 *  (intent + comment + element selector + page URL). Pure — easy to unit-test. */
export function formatHeadedAnnotationForAgent(a: HeadedAnnotation): string {
  const intent = a.intent === 'question' ? 'Question about' : 'Please change'
  const sel = a.payload?.target?.selector ?? a.payload?.target?.tagName ?? 'the selected element'
  const url = a.payload?.page?.url ?? ''
  const comment = (a.comment ?? '').trim()
  const lines = [
    `${intent} \`${sel}\`${url ? ` on ${url}` : ''}:`,
    comment ? `\n${comment}` : ''
  ]
  return lines.join('').trim()
}
