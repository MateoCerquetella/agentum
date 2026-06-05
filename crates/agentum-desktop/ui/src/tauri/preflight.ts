// Tool + agent detection now flows through the embedded agentum-server
// (`/api/preflight/*`) instead of a parallel native Tauri command, so there's one
// backend. Consuming slices (preflight, detected-agents) are unchanged — they
// still call `api.preflight.*`; only the implementation moved to HTTP.
//
// `detectRemoteAgents` resolves the repo's SSH connection to a server host and
// reads that host's readiness (the server probes agent CLIs over SSH there), so
// the composer's remote Agent picker lists what's actually installed on the
// remote — not just "Blank Terminal". See `detectRemoteAgentsViaServer`.
import { apiUrl } from '@/runtime/server-endpoint'
import { detectRemoteAgentsViaServer } from '@/runtime/server-host-client'

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
  detectRemoteAgents: (args?: { connectionId?: string }) =>
    args?.connectionId ? detectRemoteAgentsViaServer(args.connectionId) : Promise.resolve([])
}
