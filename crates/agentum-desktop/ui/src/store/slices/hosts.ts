import type { StateCreator } from 'zustand'
import type { AppState } from '../types'
import {
  listServerHosts,
  resolveServerHostIdForConnection,
  getServerHostReadinessUname,
  type ServerHost
} from '@/runtime/server-host-client'

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
}

export type HostsSlice = {
  /** Per-host label + OS detail, keyed by HostKey. The host→repo structure is
   *  derived from the repo list; this slice holds only what isn't derivable. */
  hostMetaByKey: Record<HostKey, HostMeta>
  setHostMeta: (key: HostKey, meta: HostMeta) => void
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

  setHostMeta: (key, meta) =>
    set((s) => ({ hostMetaByKey: { ...s.hostMetaByKey, [key]: meta } })),

  hydrateHosts: async () => {
    // Local host: find the daemon's own host in the registry, read its uname.
    try {
      const hosts: ServerHost[] = await listServerHosts()
      const local = hosts.find((h) => h.kind === 'local')
      const localUname = local ? await getServerHostReadinessUname(local.id) : null
      get().setHostMeta('local', {
        key: 'local',
        kind: 'local',
        label: local?.name?.trim() || 'This Mac',
        detail: unameDetail('localhost', localUname)
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
        const hosts: ServerHost[] = await listServerHosts()
        const host = hosts.find((h) => h.id === hostId)
        const uname = await getServerHostReadinessUname(hostId)
        const prefix = host?.hostname ? `ssh ${host.hostname}` : 'ssh'
        get().setHostMeta(key, { key, kind: 'ssh', label, detail: unameDetail(prefix, uname) })
      } catch (err) {
        console.warn('[agentum] hydrateHosts: ssh host failed', connectionId, err)
      }
    }
  }
})
