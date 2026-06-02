// Additive, opt-in Option A path: connect a terminal pane to a server session
// (a tmux pane streamed over WS by the embedded agentum-server) instead of a
// local PTY. Mirrors connectPanePty's PanePtyBinding contract so the pane
// lifecycle treats both identically. Off by default — see shouldUseServerTerminals.
import type { PaneManager, ManagedPane } from '@/lib/pane-manager/pane-manager'
import { connectPanePty, type PanePtyBinding } from './pty-connection'
import type { PtyConnectionDeps } from './pty-connection-types'
import { useAppStore } from '@/store'
import { ensureWorkspaceSession } from '@/runtime/workspace-session'
import {
  bindServerSessionTerminal,
  type ServerSessionTerminalBinding
} from '@/runtime/server-session-terminal'

/** The tab's launch agent (claude/codex/…) drives the server session's tool;
 *  a plain terminal tab has none, so it runs a shell. */
function resolveSessionTool(deps: PtyConnectionDeps): string {
  const tab = useAppStore
    .getState()
    .tabsByWorktree[deps.worktreeId]?.find((t) => t.id === deps.tabId)
  return tab?.launchAgent ?? 'terminal'
}

/**
 * Opt-in (default OFF) until the server terminal path is verified live. Set
 * `localStorage['agentum.serverTerminals'] = '1'` (and reload) to route new
 * terminals through the embedded server's tmux sessions — the Option A path
 * that lets SSH/remote sessions survive disconnection. Hard failures fall back
 * to the local PTY automatically.
 */
export function shouldUseServerTerminals(): boolean {
  try {
    return globalThis.localStorage?.getItem('agentum.serverTerminals') === '1'
  } catch {
    return false
  }
}

/**
 * Drop-in alternative to `connectPanePty`: ensure a server session exists for
 * this pane's workspace (workdir) and bind its tmux pane to the xterm. If the
 * server path can't establish (no workdir, session/stream failure), it falls
 * back to `connectPanePty` so the pane still works — the proven local path.
 */
export function connectPaneServerSession(
  pane: ManagedPane,
  manager: PaneManager,
  deps: PtyConnectionDeps
): PanePtyBinding {
  let disposed = false
  let binding: ServerSessionTerminalBinding | null = null
  // When the server path fails, we hand the pane to connectPanePty and delegate
  // every binding method to it — the lifecycle hook never knows the difference.
  let localFallback: PanePtyBinding | null = null

  const fallBackToLocal = (reason: string): void => {
    if (disposed || localFallback) {
      return
    }
    console.warn(`[agentum] server terminal unavailable, using local PTY: ${reason}`)
    localFallback = connectPanePty(pane, manager, deps)
  }

  const workdir = deps.cwd ?? ''
  if (!workdir) {
    fallBackToLocal('no workdir')
  } else {
    const tool = resolveSessionTool(deps)
    void (async () => {
      try {
        const session = await ensureWorkspaceSession({ workdir, tool })
        if (disposed) {
          return
        }
        binding = await bindServerSessionTerminal(session.id, pane.terminal)
        if (disposed) {
          binding.dispose()
          binding = null
        }
      } catch (error) {
        if (!disposed) {
          fallBackToLocal(String(error))
        }
      }
    })()
  }

  return {
    dispose: () => {
      disposed = true
      binding?.dispose()
      binding = null
      localFallback?.dispose()
      localFallback = null
    },
    // Delegate to the local binding when we fell back; no-ops for the server
    // path (the server owns pane lifecycle / process tracking).
    syncRendererOutputVisibility: () => localFallback?.syncRendererOutputVisibility(),
    syncProcessTracking: () => localFallback?.syncProcessTracking()
  }
}
