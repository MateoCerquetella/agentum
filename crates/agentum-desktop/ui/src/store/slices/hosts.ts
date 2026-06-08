import type { StateCreator } from 'zustand'
import type { AppState } from '../types'
import {
  listServerHosts,
  resolveServerHostIdForConnection,
  getServerHostReadinessInfo,
  type ServerHost
} from '@/runtime/server-host-client'
import { listSessions } from '@/runtime/agentum-server-client'
import { hasTmuxForHost } from '@/components/sidebar/worktree-list-groups'

/** Stable key identifying a host in the sidebar tree. `local` for the daemon's
 *  own machine; `ssh:<connectionId>` for a remote repo's native SSH target. */
export type HostKey = string

export type HostMeta = {
  key: HostKey
  kind: 'local' | 'ssh'
  /** Display name on the host header (e.g. "studio", "forge"). */
  label: string
  /** Right-of-name OS line, e.g. "localhost · Darwin 24.5" or
   *  "ssh forge.lan · Linux 6.9". Undefined until readiness resolves. */
  detail?: string
  /** Whether `tmux` is installed on the host. Sessions run inside tmux there,
   *  so this is the "tmux available" signal on the host header (and the hook for
   *  an install prompt when false). Undefined until readiness resolves. */
  tmuxInstalled?: boolean
}

export type HostsSlice = {
  /** Per-host label + OS detail, keyed by HostKey. The host→repo structure is
   *  derived from the repo list; this slice holds only what isn't derivable. */
  hostMetaByKey: Record<HostKey, HostMeta>
  setHostMeta: (key: HostKey, meta: HostMeta) => void
  /** Host keys that currently have at least one RUNNING session backed by a real
   *  tmux session (`tmux_target` non-null). The truthful "in tmux right now"
   *  per-host signal — refreshed on the same triggers as host metadata. */
  hostsWithTmux: Set<HostKey>
  /** Populate label + OS detail for the local host and every known SSH target.
   *  Best-effort: never throws into the UI. */
  hydrateHosts: () => Promise<void>
}

/** Compose a host's OS detail line: `<transport> · <uname>` when the readiness
 *  probe returned a uname (e.g. "localhost · Darwin 24.5"), degrading to just the
 *  transport prefix when it's unknown — never a dangling separator. */
export function unameDetail(prefix: string, uname: string | null): string {
  return uname ? `${prefix} · ${uname}` : prefix
}

export const createHostsSlice: StateCreator<AppState, [], [], HostsSlice> = (set, get) => ({
  hostMetaByKey: {},
  hostsWithTmux: new Set<HostKey>(),

  setHostMeta: (key, meta) =>
    set((s) => ({ hostMetaByKey: { ...s.hostMetaByKey, [key]: meta } })),

  hydrateHosts: async () => {
    // Build server-host-id → sidebar-host-key as we resolve each host, so the
    // tmux pass below can bucket a session's `host_id` (a server UUID) under the
    // right sidebar key (`local` / `ssh:<connectionId>`).
    const serverHostIdToHostKey = new Map<string, string>()

    // Local host: find the daemon's own host in the registry, read its uname.
    try {
      const hosts: ServerHost[] = await listServerHosts()
      const local = hosts.find((h) => h.kind === 'local')
      if (local) {
        serverHostIdToHostKey.set(local.id, 'local')
      }
      const localInfo = local
        ? await getServerHostReadinessInfo(local.id)
        : { uname: null as string | null, tmuxInstalled: undefined }
      get().setHostMeta('local', {
        key: 'local',
        kind: 'local',
        label: local?.name?.trim() || 'This Mac',
        detail: unameDetail('localhost', localInfo.uname),
        tmuxInstalled: localInfo.tmuxInstalled
      })
    } catch (err) {
      console.warn('[agentum] hydrateHosts: local host failed', err)
    }

    // SSH hosts: one entry per known native target (label from the store).
    const labels = get().sshTargetLabels
    for (const [connectionId, label] of labels) {
      const key = `ssh:${connectionId}`
      // Seed the label immediately so the header renders before readiness lands.
      get().setHostMeta(key, { key, kind: 'ssh', label })
      try {
        const hostId = await resolveServerHostIdForConnection(connectionId)
        if (!hostId) continue
        serverHostIdToHostKey.set(hostId, key)
        const hosts: ServerHost[] = await listServerHosts()
        const host = hosts.find((h) => h.id === hostId)
        const info = await getServerHostReadinessInfo(hostId)
        const prefix = host?.hostname ? `ssh ${host.hostname}` : 'ssh'
        get().setHostMeta(key, {
          key,
          kind: 'ssh',
          label,
          detail: unameDetail(prefix, info.uname),
          tmuxInstalled: info.tmuxInstalled
        })
      } catch (err) {
        console.warn('[agentum] hydrateHosts: ssh host failed', connectionId, err)
      }
    }

    // Truthful per-host tmux: which hosts actually have a live tmux-backed
    // session right now. Best-effort — a session-list failure leaves the prior
    // set untouched rather than blanking the glyph.
    try {
      const sessions = await listSessions()
      const hostKeys = new Set<HostKey>([
        'local',
        ...[...labels.keys()].map((id) => `ssh:${id}`)
      ])
      const next = new Set<HostKey>()
      for (const hostKey of hostKeys) {
        if (hasTmuxForHost(sessions, hostKey, serverHostIdToHostKey)) {
          next.add(hostKey)
        }
      }
      set({ hostsWithTmux: next })
    } catch (err) {
      console.warn('[agentum] hydrateHosts: session tmux probe failed', err)
    }
  }
})
