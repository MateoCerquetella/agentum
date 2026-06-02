// Resolves the embedded agentum-server endpoint (booted in-process by the Tauri
// shell) and exposes the base URL + a boot-time health probe. The desktop drives
// the same HTTP/WS core as the TUI; see crates/agentum-desktop/src/commands/server.rs.
import { invoke } from '@tauri-apps/api/core'

export type ServerEndpoint = {
  url: string
  token: string | null
}

let cached: ServerEndpoint | null = null
let pending: Promise<ServerEndpoint> | null = null

/** Fetch (once) the embedded server's base URL + token from the Tauri shell. */
export async function getServerEndpoint(): Promise<ServerEndpoint> {
  if (cached) {
    return cached
  }
  if (!pending) {
    pending = invoke<ServerEndpoint>('app_get_server_endpoint').then((endpoint) => {
      cached = endpoint
      return endpoint
    })
  }
  return pending
}

function joinPath(base: string, path: string): string {
  return `${base.replace(/\/$/, '')}${path.startsWith('/') ? path : `/${path}`}`
}

/** Build an absolute `/api/*` URL against the embedded server. */
export async function apiUrl(path: string): Promise<string> {
  const { url } = await getServerEndpoint()
  return joinPath(url, path)
}

/** Build a `ws://` URL against the embedded server. */
export async function wsUrl(path: string): Promise<string> {
  const { url } = await getServerEndpoint()
  return joinPath(url.replace(/^http/, 'ws'), path)
}

/** One-time connectivity probe; logs the embedded server's health on boot. */
export async function probeEmbeddedServer(): Promise<boolean> {
  try {
    const endpoint = await getServerEndpoint()
    const res = await fetch(joinPath(endpoint.url, '/api/health'))
    console.info(`[agentum] embedded server ${endpoint.url} health: ${res.status}`)
    return res.ok
  } catch (error) {
    console.warn('[agentum] embedded server health probe failed:', error)
    return false
  }
}
