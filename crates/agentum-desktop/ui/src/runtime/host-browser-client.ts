// Typed client for the host-browser routes on the embedded agentum-server
// (`/api/host-browser/*`, spec 009a). Mirrors `harness-client.ts`: built on the
// loopback endpoint from `server-endpoint.ts`. Wire shapes track
// `crates/agentum-server/src/routes/host_browser.rs` + `host_browser.rs` (serde
// snake_case) so there is one source of truth.
//
// This is the DIRECT-WS transport (009a Phase 3): the screencast WS speaks the
// `0x62` binary frame protocol out and the scratch input JSON in — no
// runtime-environments RPC broker (that path was removed in spec 007).
import {
  decodeBrowserScreencastFrame,
  type BrowserScreencastFrame
} from '../shared/browser-screencast-protocol'
import { apiUrl, getServerEndpoint, wsUrl } from './server-endpoint'

/** Result of `POST /api/host-browser` — start or re-attach. */
export type StartedHostBrowser = {
  id: string
  attached: boolean
  mac_port: number
  cdp_host_port: number
}

/** `GET /api/host-browser/{id}` status snapshot. */
export type HostBrowserStatus = {
  id: string
  mac_port: number
  cdp_host_port: number
  tmux_running: boolean
  cdp_reachable: boolean
}

/** Input messages the screencast WS accepts (scratch protocol → CDP). */
export type HostBrowserInput =
  | { type: 'mouse'; action: 'move' | 'down' | 'up'; x: number; y: number; button?: 'left' | 'middle' | 'right' }
  | { type: 'wheel'; x: number; y: number; dx: number; dy: number }
  | { type: 'key'; key: string }
  | { type: 'navigate'; url: string }

async function authHeaders(): Promise<Record<string, string>> {
  const { token } = await getServerEndpoint()
  return token ? { Authorization: `Bearer ${token}` } : {}
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const url = await apiUrl(path)
  const res = await fetch(url, {
    ...init,
    headers: {
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      ...(await authHeaders()),
      ...(init?.headers ?? {})
    }
  })
  if (!res.ok) {
    const detail = await res.text().catch(() => '')
    throw new Error(`host-browser ${res.status} on ${path}${detail ? ` — ${detail}` : ''}`)
  }
  const text = await res.text()
  return (text ? JSON.parse(text) : undefined) as T
}

/** `POST /api/host-browser` — launch (or re-attach to) the host browser for a worktree. */
export function startHostBrowser(hostId: string, workdir: string): Promise<StartedHostBrowser> {
  return request('/api/host-browser', {
    method: 'POST',
    body: JSON.stringify({ host_id: hostId, workdir })
  })
}

/** `GET /api/host-browser/{id}` — status (running / current tunnel / reachable). */
export function getHostBrowserStatus(id: string): Promise<HostBrowserStatus> {
  return request(`/api/host-browser/${encodeURIComponent(id)}`)
}

/** `POST /api/host-browser/{id}/navigate` — point the host browser at `url`. */
export function navigateHostBrowser(id: string, url: string): Promise<void> {
  return request(`/api/host-browser/${encodeURIComponent(id)}/navigate`, {
    method: 'POST',
    body: JSON.stringify({ url })
  })
}

/** `DELETE /api/host-browser/{id}` — kill the host browser + forget it. */
export function stopHostBrowser(id: string): Promise<void> {
  return request(`/api/host-browser/${encodeURIComponent(id)}`, { method: 'DELETE' })
}

/** Live screencast handle: push input in, frames flow out via the `onFrame` callback. */
export type HostBrowserScreencast = {
  sendInput: (msg: HostBrowserInput) => void
  close: () => void
}

/**
 * Open `WS /api/host-browser/{id}/screencast`: decode each `0x62` binary frame
 * and forward it to `onFrame`; `sendInput` serializes the scratch input protocol
 * back. Auto-reconnects with capped backoff (the host browser outlives the
 * socket, so a drop is always recoverable). The token rides in `?token=` because
 * browsers can't set headers on a WS upgrade.
 */
export async function openHostBrowserScreencast(
  id: string,
  handlers: {
    onFrame: (frame: BrowserScreencastFrame) => void
    onOpen?: () => void
    onClose?: () => void
  }
): Promise<HostBrowserScreencast> {
  const { token } = await getServerEndpoint()
  const base = await wsUrl(`/api/host-browser/${encodeURIComponent(id)}/screencast`)
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
    sock.binaryType = 'arraybuffer'
    ws = sock
    sock.addEventListener('open', () => {
      attempt = 0
      handlers.onOpen?.()
    })
    sock.addEventListener('message', (event) => {
      if (!(event.data instanceof ArrayBuffer)) return
      const frame = decodeBrowserScreencastFrame(new Uint8Array(event.data))
      if (frame) {
        handlers.onFrame(frame)
      }
    })
    sock.addEventListener('close', () => {
      if (sock !== ws || disposed) return
      handlers.onClose?.()
      attempt += 1
      timer = setTimeout(connect, backoffMs(attempt))
    })
  }

  connect()

  return {
    sendInput: (msg: HostBrowserInput) => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify(msg))
      }
    },
    close: () => {
      disposed = true
      if (timer) clearTimeout(timer)
      ws?.close()
    }
  }
}
