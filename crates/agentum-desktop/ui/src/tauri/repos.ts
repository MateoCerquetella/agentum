// The repo registry + git-ref logic moved off native Tauri commands into the
// embedded agentum-server (`/api/repos/*`); this namespace now calls the server
// (server-repo-client) so the registry has one owner the TUI/dashboard share.
// EXCEPTION: the folder-picker dialog stays native (it needs a Tauri window).
import { call, subscribe } from './core'
import type { AgentumApi } from './contract'
import {
  reposList,
  reposAdd,
  reposAddRemote,
  reposUpdate,
  reposCreate,
  reposClone,
  reposRemove,
  reposReorder,
  getServerRepoBaseRefDefault,
  getServerRepoBaseRefs,
  getServerRepoBaseRefDetails
} from '../runtime/server-repo-client'
import { resolveServerHostIdForConnection } from '../runtime/server-host-client'

export const repos = {
  list: () => reposList(),
  add: (...args: any[]) => reposAdd(args[0]?.path, args[0]?.kind),
  update: (...args: any[]) => reposUpdate(args[0]?.repoId, args[0]?.updates ?? {}),
  create: (...args: any[]) =>
    reposCreate({
      parentPath: args[0]?.parentPath,
      name: args[0]?.name,
      kind: args[0]?.kind
    }),
  clone: (...args: any[]) => reposClone(args[0]?.url, args[0]?.destination),
  remove: (...args: any[]) => reposRemove(args[0]?.repoId),
  reorder: (...args: any[]) => reposReorder(args[0]?.orderedIds ?? []),
  getBaseRefDefault: (...args: any[]) => getServerRepoBaseRefDefault(args[0]?.repoId),
  searchBaseRefs: (...args: any[]) =>
    getServerRepoBaseRefs(args[0]?.repoId, args[0]?.query ?? '', args[0]?.limit ?? 20),
  searchBaseRefDetails: (...args: any[]) =>
    getServerRepoBaseRefDetails(args[0]?.repoId, args[0]?.query ?? '', args[0]?.limit ?? 20),
  // Native folder dialog stays in the desktop shell (needs a Tauri window).
  pickFolder: (...args: any[]) => call('repos_pick_folder', args),
  pickDirectory: (...args: any[]) => call('repos_pick_directory', args),
  // Register a remote (SSH) repo: resolve the connection to a server host id
  // and route through /api/repos so the repo persists connectionId + hostId
  // (server git/worktree/agent ops then run over SSH). Was a fixed "not
  // available" stub; the AddRepoSteps wizard calls reposAddRemote directly,
  // this is the same path for the folder / non-git dialogs.
  addRemote: async (...args: any[]) => {
    const connectionId: string = args[0]?.connectionId ?? ''
    const remotePath: string = args[0]?.remotePath ?? ''
    if (!connectionId || !remotePath) {
      return { error: 'a remote project needs both an SSH connection and a path' }
    }
    const hostId = await resolveServerHostIdForConnection(connectionId)
    return reposAddRemote(connectionId, remotePath, hostId)
  },
  cloneAbort: () => Promise.resolve(),
  // These Tauri events were never emitted by the native commands (no-op today);
  // kept for API parity. Source-control surfaces refresh by re-fetching.
  onChanged: (cb: (p: any) => void) => subscribe('repos-changed', cb),
  onCloneProgress: (cb: (p: any) => void) => subscribe('repos-clone-progress', cb)
} satisfies AgentumApi['repos']
