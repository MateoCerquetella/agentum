// Tool + agent detection now flows through the embedded agentum-server
// (`/api/preflight/*`) instead of a parallel native Tauri command, so there's one
// backend. Consuming slices (preflight, detected-agents) are unchanged — they
// still call `api.preflight.*`; only the implementation moved to HTTP.
//
// `detectRemoteAgents` resolves the repo's SSH connection to a server host and
// reads that host's readiness (the server probes agent CLIs over SSH there), so
// the composer's remote Agent picker lists what's actually installed on the
// remote — not just "Blank Terminal". See `detectRemoteAgentsViaServer`.
import { getJson } from '@/runtime/server-http'

export const preflight = {
  check: () => getJson('/api/preflight/check'),
  detectAgents: () => getJson('/api/preflight/agents'),
  refreshAgents: () => getJson('/api/preflight/agents/refresh'),
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
