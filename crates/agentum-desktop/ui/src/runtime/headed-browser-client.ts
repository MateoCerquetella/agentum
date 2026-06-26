// Client for the "Open Browser (persistent)" surface: a real HEADED Chrome
// window the agent drives over CDP (see the headed-agent-browser design spec).
// Unlike the screencast client, there is no stream — Chrome paints its own OS
// window natively; this just asks the embedded server to launch/stop it and
// reports the CDP port the agentum MCP attaches to.
import { apiUrl, getServerEndpoint } from './server-endpoint'

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
