// Bridges the desktop's native SSH targets (ssh.rs / store.sshTargets) to the
// embedded server's host registry (`/api/hosts`), which is the source of truth
// for where a session's tmux pane runs. A remote repo carries a `connectionId`
// (a native SSH target id); to run its sessions on the remote we need the
// matching server host id. This module create-or-gets that host by SSH
// coordinates and caches the mapping for the session lifetime.
import { getJson, postJson } from './server-http'
import { api } from '@/tauri'
import type { SshTarget } from '../../../shared/ssh-types'

/** A server host as returned by `/api/hosts` (camelCase flattened kind). */
type ServerHost = {
  id: string
  name: string
  kind: 'local' | 'ssh'
  user?: string
  hostname?: string
  port?: number
}

/** `GET /api/hosts` — the registered hosts (local + ssh). */
export function listServerHosts(): Promise<ServerHost[]> {
  return getJson<ServerHost[]>('/api/hosts')
}

/** `POST /api/hosts` — register an SSH host. Auth defaults to agent/key on the
 *  server; we pass an explicit key path when the native target has one so the
 *  remote exec uses the same identity the user configured. */
function createServerHost(name: string, target: SshTarget): Promise<ServerHost> {
  const auth = target.identityFile
    ? { auth: 'key', path: target.identityFile }
    : { auth: 'agent' }
  return postJson<ServerHost>('/api/hosts', {
    name,
    kind: 'ssh',
    user: target.username,
    hostname: target.host,
    port: target.port,
    ...auth
  })
}

// connectionId (native ssh target id) → resolved server host id. Bounded by the
// number of distinct SSH targets the user has; cleared only on reload.
const hostIdByConnectionId = new Map<string, string | null>()

function sameSshCoords(host: ServerHost, target: SshTarget): boolean {
  return (
    host.kind === 'ssh' &&
    host.user === target.username &&
    host.hostname === target.host &&
    (host.port ?? 22) === (target.port ?? 22)
  )
}

/**
 * Resolve a repo's `connectionId` to a server host id, creating the host from
 * the native SSH target if one doesn't already exist. Returns `null` when the
 * connection can't be resolved (unknown target) so callers fall back to a local
 * session rather than failing the pane. Cached per connectionId.
 */
export async function resolveServerHostIdForConnection(
  connectionId: string
): Promise<string | null> {
  if (hostIdByConnectionId.has(connectionId)) {
    return hostIdByConnectionId.get(connectionId) ?? null
  }
  try {
    // The store only keeps target labels, so fetch full coords natively.
    const targets = (await api.ssh.listTargets()) as SshTarget[]
    const target = targets.find((t) => t.id === connectionId)
    if (!target) {
      hostIdByConnectionId.set(connectionId, null)
      return null
    }
    const hosts = await listServerHosts()
    const existing = hosts.find((h) => sameSshCoords(h, target))
    const host = existing ?? (await createServerHost(target.label || target.host, target))
    hostIdByConnectionId.set(connectionId, host.id)
    return host.id
  } catch (err) {
    console.warn('[agentum] failed to resolve server host for connection', connectionId, err)
    // Don't poison the cache on a transient failure — allow a later retry.
    return null
  }
}
