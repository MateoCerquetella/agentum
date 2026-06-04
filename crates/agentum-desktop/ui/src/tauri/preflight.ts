// Tool + agent detection now flows through the embedded agentum-server
// (`/api/preflight/*`) instead of a parallel native Tauri command, so there's one
// backend. Consuming slices (preflight, detected-agents) are unchanged — they
// still call `api.preflight.*`; only the implementation moved to HTTP.
//
// `detectRemoteAgents` stays a no-op ([]): the desktop's SSH targets aren't the
// daemon's hosts, so per-target remote detection can't route through the server
// yet (it was already a native stub returning []).
import { apiUrl } from '@/runtime/server-endpoint'

async function serverGet<T>(path: string): Promise<T> {
  const res = await fetch(await apiUrl(path))
  if (!res.ok) {
    throw new Error(`agentum-server ${res.status} on ${path}`)
  }
  return (await res.json()) as T
}

export const preflight = {
  check: () => serverGet('/api/preflight/check'),
  detectAgents: () => serverGet('/api/preflight/agents'),
  refreshAgents: () => serverGet('/api/preflight/agents/refresh'),
  detectRemoteAgents: () => Promise.resolve([])
}
