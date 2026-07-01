// Bridges the desktop's native SSH targets (ssh.rs / store.sshTargets) to the
// embedded server's host registry (`/api/hosts`), which is the source of truth
// for where a session's tmux pane runs. A remote repo carries a `connectionId`
// (a native SSH target id); to run its sessions on the remote we need the
// matching server host id. This module create-or-gets that host by SSH
// coordinates and caches the mapping for the session lifetime.
import { del, getJson, postJson, putJson } from './server-http'
import { api } from '@/tauri'
import type { SshTarget } from '../../../shared/ssh-types'

/** A server host as returned by `/api/hosts` (camelCase flattened kind). */
export type ServerHost = {
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

/** One agent CLI entry from `/api/hosts/{id}/readiness` (`agents[]`). */
type HostReadinessAgent = { id: string; installed: boolean }
type HostReadinessResp = { agents?: HostReadinessAgent[] }

/**
 * Agent CLIs installed on the host behind a repo's SSH `connectionId`, for
 * the composer's remote Agent picker. Resolves connectionId → server host id
 * (same mapping sessions use), then reads the host's readiness report — the
 * server already probes "every probed agent CLI" over SSH there — and returns
 * the installed ids. Returns `[]` (so the picker falls back to Blank Terminal)
 * when the host can't be resolved or reached, never throwing into the UI.
 */
export async function detectRemoteAgentsViaServer(connectionId: string): Promise<string[]> {
  try {
    const hostId = await resolveServerHostIdForConnection(connectionId)
    if (!hostId) {
      return []
    }
    const readiness = await getJson<HostReadinessResp>(
      `/api/hosts/${encodeURIComponent(hostId)}/readiness`
    )
    return (readiness.agents ?? []).filter((a) => a.installed).map((a) => a.id)
  } catch (err) {
    console.warn('[agentum] failed to detect remote agents for connection', connectionId, err)
    return []
  }
}

/**
 * Read a host's OS one-liner from `/api/hosts/{id}/readiness` (`system.uname`,
 * e.g. "Linux 6.9" / "Darwin 24.5"). Best-effort — returns null on any failure
 * or when the daemon predates the field, so the sidebar header degrades to a
 * kind-only label rather than throwing.
 */
export async function getServerHostReadinessUname(hostId: string): Promise<string | null> {
  return (await getServerHostReadinessInfo(hostId)).uname
}

/** Host readiness essentials for the sidebar header: OS one-liner + whether
 *  `tmux` is installed (sessions run inside tmux on the host, so this is the
 *  "tmux is available here" signal). One `/readiness` fetch; best-effort —
 *  unknowns degrade to null/undefined rather than throwing into the UI. */
export type HostReadinessInfo = { uname: string | null; tmuxInstalled?: boolean }

export async function getServerHostReadinessInfo(hostId: string): Promise<HostReadinessInfo> {
  try {
    const readiness = await getJson<{
      system?: { uname?: string | null }
      required?: { id: string; installed: boolean }[]
    }>(`/api/hosts/${encodeURIComponent(hostId)}/readiness`)
    return {
      uname: readiness.system?.uname ?? null,
      tmuxInstalled: readiness.required?.find((d) => d.id === 'tmux')?.installed
    }
  } catch (err) {
    console.warn('[agentum] failed to read host readiness', hostId, err)
    return { uname: null }
  }
}

/** Resolve a sidebar host key (`local` | `ssh:<connectionId>`) to a server host
 *  id, so the readiness dialog can talk to `/api/hosts/{id}/…`. */
export async function resolveServerHostIdForHostKey(hostKey: string): Promise<string | null> {
  if (hostKey.startsWith('ssh:')) {
    return resolveServerHostIdForConnection(hostKey.slice('ssh:'.length))
  }
  const hosts = await listServerHosts()
  return hosts.find((h) => h.kind === 'local')?.id ?? null
}

/** One pane of a discovered (non-agentum) tmux session on a host. */
export type DiscoveredTmuxPane = { command: string; cwd: string }

/** A tmux session running on a host that agentum does not manage, as returned
 *  by `GET /api/hosts/{id}/tmux-sessions`. `related` is true when any pane's
 *  cwd is at or under the `path` passed in the query. */
export type DiscoveredTmuxSession = {
  name: string
  attached: boolean
  created_at?: number | null
  panes: DiscoveredTmuxPane[]
  related: boolean
}

/** `GET /api/hosts/{id}/tmux-sessions?path=…&all=true` — tmux sessions on the
 *  host. Pass `all=true` to include agentum-managed sessions. */
export function listHostTmuxSessions(
  hostId: string,
  opts?: { path?: string; all?: boolean }
): Promise<DiscoveredTmuxSession[]> {
  const params = new URLSearchParams()
  if (opts?.path) params.set('path', opts.path)
  if (opts?.all) params.set('all', 'true')
  const query = params.toString() ? `?${params.toString()}` : ''
  return getJson<DiscoveredTmuxSession[]>(
    `/api/hosts/${encodeURIComponent(hostId)}/tmux-sessions${query}`
  )
}

/** `DELETE /api/hosts/{id}/tmux-sessions/{name}` — kill a tmux session on the
 *  host. Only works for non-agentum-managed sessions. */
export function killHostTmuxSession(hostId: string, name: string): Promise<void> {
  return del(`/api/hosts/${encodeURIComponent(hostId)}/tmux-sessions/${encodeURIComponent(name)}`)
}

/** `POST /api/hosts/{id}/tmux-sessions/{name}/attach` — bind a discovered tmux
 *  session to an agentum session record (running, externally-flagged) so it can
 *  stream like any managed session. Idempotent per (host, tmux name). Returns
 *  the server Session wire shape (see runtime/agentum-server-client.ts). */
export function attachHostTmuxSession(
  hostId: string,
  name: string
): Promise<import('./agentum-server-client').Session> {
  return postJson(
    `/api/hosts/${encodeURIComponent(hostId)}/tmux-sessions/${encodeURIComponent(name)}/attach`
  )
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
  // Lazy import to avoid circular dependency: server-host-client ← store ← hosts ← server-host-client.
  const { useAppStore } = await import('@/store')
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

/** Extract the native SSH target id from a session's hostKey
 *  (`'ssh:<connectionId>'`). The connectionId IS the target id (see
 *  resolveServerHostIdForConnection), which is also how `sshConnectionStates` is
 *  keyed — so a stream's recovery can reconcile the badge with no host-id round
 *  trip. Returns null for local sessions or a malformed key. */
function sshConnectionIdFromHostKey(hostKey: string | undefined): string | null {
  if (!hostKey || !hostKey.startsWith('ssh:')) {
    return null
  }
  const id = hostKey.slice('ssh:'.length)
  return id || null
}

/**
 * Mark the SSH target behind a session's hostKey CONNECTED after its stream
 * recovered from a transient drop. A live re-attach is itself the reachability
 * proof — no extra probe needed — so this mirrors connectSshTargetViaServer's
 * success state without the network round trip. The flip to 'connected' both
 * repaints the status-bar/sidebar badges and bumps sshConnectedGeneration, which
 * re-fires the file explorer's failed-load retry — so a recovered host's tree
 * refreshes instead of staying stuck on the outage's error (the reported bug).
 * No-op for local sessions.
 */
export async function markHostConnectedFromHostKey(hostKey: string | undefined): Promise<void> {
  const connectionId = sshConnectionIdFromHostKey(hostKey)
  if (!connectionId) {
    return
  }
  const { useAppStore } = await import('@/store')
  useAppStore.getState().setSshConnectionState(connectionId, {
    targetId: connectionId,
    status: 'connected',
    error: null,
    reconnectAttempt: 0
  })
}

/**
 * Mark the SSH target behind a session's hostKey RECONNECTING when its stream
 * drops — but only when we currently consider it connected. Two reasons: it
 * keeps the badge honest during an outage, and it makes the NEXT recovery a real
 * 'reconnecting' → 'connected' transition so sshConnectedGeneration bumps again.
 * Without it, a second outage would be a no-op 'connected' → 'connected' write
 * that never re-triggers the file-tree retry. We never fabricate a badge for a
 * host the UI never tracked, nor stomp an in-flight explicit connect.
 */
export async function markHostReconnectingFromHostKey(hostKey: string | undefined): Promise<void> {
  const connectionId = sshConnectionIdFromHostKey(hostKey)
  if (!connectionId) {
    return
  }
  const { useAppStore } = await import('@/store')
  const store = useAppStore.getState()
  if (store.sshConnectionStates.get(connectionId)?.status !== 'connected') {
    return
  }
  store.setSshConnectionState(connectionId, {
    targetId: connectionId,
    status: 'reconnecting',
    error: null,
    reconnectAttempt: 1
  })
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

// `auth` is an internally-tagged SshAuth on the server (tag = "auth"), so it
// must be a nested object — {auth:'agent'}, {auth:'key', path}, or
// {auth:'password', password} — NOT a flat field. Precedence: an explicit
// password wins (host only allows password login), then an identity file, else
// the SSH agent.
function authFromTarget(target: SshTarget) {
  return target.password
    ? { auth: 'password' as const, password: target.password }
    : target.identityFile
      ? { auth: 'key' as const, path: target.identityFile }
      : { auth: 'agent' as const }
}

function hostBody(name: string, target: SshTarget) {
  return {
    name,
    kind: 'ssh' as const,
    user: target.username,
    hostname: targetHostname(target),
    port: target.port,
    auth: authFromTarget(target)
  }
}

function createServerHost(name: string, target: SshTarget): Promise<ServerHost> {
  return postJson<ServerHost>('/api/hosts', hostBody(name, target))
}

// PUT the host's connection settings — crucially its auth — so a re-entered or
// changed password (or key) on the target reaches the host the daemon actually
// authenticates with. The host's stored secret isn't returned by GET, so we
// can't diff it; we always push the target's current auth. This closes the bug
// where a host, once created with a wrong password, kept it forever because the
// resolver matched by host/user/port only and never refreshed the secret.
function updateServerHost(id: string, name: string, target: SshTarget): Promise<ServerHost> {
  return putJson<ServerHost>(`/api/hosts/${encodeURIComponent(id)}`, hostBody(name, target))
}

/**
 * Push a saved target's auth to its matching server host so a re-entered or
 * changed password takes effect immediately. Updates an existing host only —
 * it does not eagerly create one (the resolver creates it lazily on first
 * session, with this same current auth). Best-effort: never throws into the
 * save flow.
 */
export async function syncServerHostAuthForTarget(target: SshTarget): Promise<void> {
  try {
    const hosts = await listServerHosts()
    const existing = hosts.find((h) => sameSshCoords(h, target))
    if (!existing) {
      return
    }
    const host = await updateServerHost(existing.id, target.label || target.host, target)
    hostIdByConnectionId.set(target.id, host.id)
  } catch (err) {
    console.warn('[agentum] failed to sync server host auth for target', target.id, err)
  }
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
    // Refresh an existing host's auth from the target (not just reuse it) so a
    // password changed since the host was created actually takes effect.
    const host = existing
      ? await updateServerHost(existing.id, target.label || target.host, target)
      : await createServerHost(target.label || target.host, target)
    hostIdByConnectionId.set(connectionId, host.id)
    return host.id
  } catch (err) {
    console.warn('[agentum] failed to resolve server host for connection', connectionId, err)
    // Don't poison the cache on a transient failure — allow a later retry.
    return null
  }
}
