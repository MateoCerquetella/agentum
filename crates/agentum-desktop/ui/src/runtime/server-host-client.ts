// Bridges the desktop's native SSH targets (ssh.rs / store.sshTargets) to the
// embedded server's host registry (`/api/hosts`), which is the source of truth
// for where a session's tmux pane runs. A remote repo carries a `connectionId`
// (a native SSH target id); to run its sessions on the remote we need the
// matching server host id. This module create-or-gets that host by SSH
// coordinates and caches the mapping for the session lifetime.
import { getJson, postJson, putJson } from './server-http'
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

/** `GET /api/hosts/{id}/tmux-sessions?path=…` — tmux sessions on the host that
 *  agentum does not manage. One SSH round trip; "no tmux server" is `[]`. */
export function listHostTmuxSessions(
  hostId: string,
  path?: string
): Promise<DiscoveredTmuxSession[]> {
  const query = path ? `?path=${encodeURIComponent(path)}` : ''
  return getJson<DiscoveredTmuxSession[]>(
    `/api/hosts/${encodeURIComponent(hostId)}/tmux-sessions${query}`
  )
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
