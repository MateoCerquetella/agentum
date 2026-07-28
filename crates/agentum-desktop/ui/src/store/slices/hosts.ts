import type { StateCreator } from 'zustand'
import type { AppState } from '../types'
import {
  listServerHosts,
  listHostTmuxSessions,
  resolveServerHostIdForConnection,
  getServerHostReadinessInfo,
  type DiscoveredTmuxSession,
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
  /** Secondary host line. Local includes its uname; SSH uses only a friendly
   *  OS family such as "Linux" or "macOS". Undefined until readiness resolves. */
  detail?: string
  /** Whether `tmux` is installed on the host. Sessions run inside tmux there,
   *  so this is the "tmux available" signal on the host header (and the hook for
   *  an install prompt when false). Undefined until readiness resolves. */
  tmuxInstalled?: boolean
}

/** Discovered (non-agentum) tmux sessions on one SSH host, for the sidebar's
 *  "Remote tmux" section. `sessions` persists across refreshes so the list
 *  doesn't flicker empty while a re-fetch is in flight. */
type RemoteTmuxState = {
  status: 'loading' | 'ready' | 'error'
  sessions: DiscoveredTmuxSession[]
  error?: string
  fetchedAt?: number
}

export type HostsSlice = {
  /** Per-host label + OS detail, keyed by HostKey. The host→repo structure is
   *  derived from the repo list; this slice holds only what isn't derivable. */
  hostMetaByKey: Record<HostKey, HostMeta>
  setHostMeta: (key: HostKey, meta: HostMeta) => void
  /** Populate label + OS detail for the local host and every known SSH target.
   *  Best-effort: never throws into the UI. */
  hydrateHosts: () => Promise<void>
  /** Discovered external tmux sessions per SSH host (`ssh:<connectionId>`). */
  remoteTmuxByHostKey: Record<HostKey, RemoteTmuxState>
  /** Fetch the host's non-agentum tmux sessions (one SSH round trip; no
   *  polling — call on project activation or manual refresh). `repoPath`
   *  drives the per-session `related` flag. Errors land in state, never throw. */
  fetchRemoteTmuxSessions: (connectionId: string, repoPath?: string) => Promise<void>
}

/** Compose a host's OS detail line: `<transport> · <uname>` when the readiness
 *  probe returned a uname (e.g. "localhost · Darwin 24.5"), degrading to just the
 *  transport prefix when it's unknown — never a dangling separator. */
export function unameDetail(prefix: string, uname: string | null): string {
  return uname ? `${prefix} · ${uname}` : prefix
}

/** Keep remote host subtitles intentionally terse. The header already names
 *  the SSH target, so repeating its transport, hostname, and kernel version
 *  adds noise without helping users distinguish hosts. */
export function sshOsDetail(uname: string | null): string {
  const kernel = uname?.trim().split(/\s+/, 1)[0]
  if (!kernel) return 'SSH'

  switch (kernel.toLowerCase()) {
    case 'darwin':
      return 'macOS'
    case 'linux':
      return 'Linux'
    case 'freebsd':
      return 'FreeBSD'
    case 'openbsd':
      return 'OpenBSD'
    case 'netbsd':
      return 'NetBSD'
    case 'windows_nt':
      return 'Windows'
    default:
      return /^(cygwin|mingw|msys)/i.test(kernel) ? 'Windows' : kernel
  }
}

export const createHostsSlice: StateCreator<AppState, [], [], HostsSlice> = (set, get) => ({
  hostMetaByKey: {},

  setHostMeta: (key, meta) =>
    set((s) => ({ hostMetaByKey: { ...s.hostMetaByKey, [key]: meta } })),

  remoteTmuxByHostKey: {},

  fetchRemoteTmuxSessions: async (connectionId, repoPath) => {
    const key = `ssh:${connectionId}`
    const setEntry = (entry: RemoteTmuxState): void =>
      set((s) => ({ remoteTmuxByHostKey: { ...s.remoteTmuxByHostKey, [key]: entry } }))
    const prev = get().remoteTmuxByHostKey[key]
    setEntry({ status: 'loading', sessions: prev?.sessions ?? [], fetchedAt: prev?.fetchedAt })
    try {
      const hostId = await resolveServerHostIdForConnection(connectionId)
      if (!hostId) {
        setEntry({ status: 'error', sessions: [], error: 'host not registered' })
        return
      }
      const sessions = await listHostTmuxSessions(hostId, { path: repoPath })
      setEntry({ status: 'ready', sessions, fetchedAt: Date.now() })
    } catch (err) {
      setEntry({
        status: 'error',
        sessions: prev?.sessions ?? [],
        error: err instanceof Error ? err.message : String(err)
      })
    }
  },

  hydrateHosts: async () => {
    // Local host: find the daemon's own host in the registry, read its uname.
    try {
      const hosts: ServerHost[] = await listServerHosts()
      const local = hosts.find((h) => h.kind === 'local')
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
        const info = await getServerHostReadinessInfo(hostId)
        get().setHostMeta(key, {
          key,
          kind: 'ssh',
          label,
          detail: sshOsDetail(info.uname),
          tmuxInstalled: info.tmuxInstalled
        })
      } catch (err) {
        console.warn('[agentum] hydrateHosts: ssh host failed', connectionId, err)
      }
    }
    // Note: the truthful per-host "in tmux right now" signal is derived
    // reactively in WorktreeList from the open-pane tmux map (tmuxByPaneKey),
    // not from the session list — closed-but-persisted tmux sessions no longer
    // mark a host. See hostKeysWithOpenTmux in worktree-list-groups.ts.
  }
})
