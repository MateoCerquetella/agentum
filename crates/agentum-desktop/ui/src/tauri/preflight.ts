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
  // Dynamic import: server-host-client pulls in `@/store` + `@/tauri`, which
  // statically importing here would weave into the barrel's init cycle. Defer
  // it to call time (this only runs when a remote project's picker opens).
  detectRemoteAgents: async (args?: { connectionId?: string }) => {
    if (!args?.connectionId) {
      return []
    }
    const { detectRemoteAgentsViaServer } = await import('@/runtime/server-host-client')
    return detectRemoteAgentsViaServer(args.connectionId)
  }
}
