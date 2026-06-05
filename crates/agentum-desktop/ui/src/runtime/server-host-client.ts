// Bridges the desktop's native SSH targets (ssh.rs / store.sshTargets) to the
// embedded server's host registry (`/api/hosts`), which is the source of truth
// for where a session's tmux pane runs. A remote repo carries a `connectionId`
// (a native SSH target id); to run its sessions on the remote we need the
// matching server host id. This module create-or-gets that host by SSH
// coordinates and caches the mapping for the session lifetime.
import { getJson, postJson } from './server-http'
import { api } from '@/tauri'
import { useAppStore } from '@/store'
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

/** Result of `POST /api/hosts/{id}/test` — a real SSH+tmux reachability probe. */
export type ServerHostProbe = {
  ok: boolean
  message: string
  uname?: string | null
  tmux: boolean
  git: boolean
}

/** `POST /api/hosts/{id}/test` — probe the host over SSH (uname + tmux + git). */
export function testServerHost(hostId: string): Promise<ServerHostProbe> {
  return postJson<ServerHostProbe>(`/api/hosts/${encodeURIComponent(hostId)}/test`)
}

/**
 * Shared "Connect" for any SSH connect button (settings, status bar, add-repo
 * remote step). The native ssh_connect transport was never ported; with the
 * server-host model "connect" means register the target as a server host and
 * probe it over SSH, then reflect the result in sshConnectionStates so every
 * SSH UI surface (dot, status-bar segment, target rows) updates. Returns a
 * human-readable result for the caller's toast.
 */
export async function connectSshTargetViaServer(
  targetId: string
): Promise<{ ok: boolean; message: string }> {
  const setState = useAppStore.getState().setSshConnectionState
  setState(targetId, { targetId, status: 'connecting', error: null, reconnectAttempt: 0 })
  try {
    const hostId = await resolveServerHostIdForConnection(targetId)
    if (!hostId) {
      throw new Error('Could not register this host with the server')
    }
    const probe = await testServerHost(hostId)
    if (probe.ok) {
      setState(targetId, { targetId, status: 'connected', error: null, reconnectAttempt: 0 })
      return {
        ok: true,
        message: probe.tmux ? 'Connected' : 'Connected — but tmux is missing on the host'
      }
    }
    const message = probe.message || 'Connection failed'
    setState(targetId, { targetId, status: 'error', error: message, reconnectAttempt: 0 })
    return { ok: false, message }
  } catch (err) {
    const message = err instanceof Error ? err.message : 'Connection failed'
    setState(targetId, { targetId, status: 'error', error: message, reconnectAttempt: 0 })
    return { ok: false, message }
  }
}

/** `POST /api/hosts` — register an SSH host. Auth defaults to agent/key on the
 *  server; we pass an explicit key path when the native target has one so the
 *  remote exec uses the same identity the user configured. */
// Why: prefer a genuine OpenSSH config alias (configHost) over the raw host so a
// config-imported target gets its ~/.ssh/config Host block. But ignore a
// configHost that just duplicates the host/IP (some imports set configHost to
// the IP itself) — it's not a real alias, and treating it as one drops the
// explicit port. The server always passes the explicit port regardless, so when
// in doubt use the literal host.
function targetHostname(target: SshTarget): string {
  const alias = target.configHost?.trim()
  if (alias && alias !== target.host) {
    return alias
  }
  return target.host
}

function createServerHost(name: string, target: SshTarget): Promise<ServerHost> {
  // `auth` is an internally-tagged SshAuth on the server (tag = "auth"), so it
  // must be a nested object — {auth:'agent'} or {auth:'key', path} — NOT a flat
  // field. Default to agent auth when the target has no explicit identity file.
  const auth = target.identityFile
    ? { auth: 'key' as const, path: target.identityFile }
    : { auth: 'agent' as const }
  return postJson<ServerHost>('/api/hosts', {
    name,
    kind: 'ssh',
    user: target.username,
    hostname: targetHostname(target),
    port: target.port,
    auth
  })
}

// connectionId (native ssh target id) → resolved server host id. Bounded by the
// number of distinct SSH targets the user has; cleared only on reload.
const hostIdByConnectionId = new Map<string, string | null>()

function sameSshCoords(host: ServerHost, target: SshTarget): boolean {
  return (
    host.kind === 'ssh' &&
    host.user === target.username &&
    host.hostname === targetHostname(target) &&
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
