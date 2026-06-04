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
import { detectAgentStatusFromTitle } from '@/lib/agent-status'
import { makePaneKey } from '../../../../shared/stable-pane-id'

/** The tab's launch agent (claude/codex/…) drives the server session's tool;
 *  a plain terminal tab has none, so it runs a shell. */
function resolveSessionTool(deps: PtyConnectionDeps): string {
  const tab = useAppStore
    .getState()
    .tabsByWorktree[deps.worktreeId]?.find((t) => t.id === deps.tabId)
  return tab?.launchAgent ?? 'terminal'
}

/**
 * Default ON — terminals run as real tmux sessions in the embedded
 * agentum-server (the local Tauri PTY path is a half-ported stub). Set
 * `localStorage['agentum.serverTerminals'] = '0'` to force the local path.
 */
export function shouldUseServerTerminals(): boolean {
  try {
    return globalThis.localStorage?.getItem('agentum.serverTerminals') !== '0'
  } catch {
    return true
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
  // Synthetic pty id so the sidebar's title-derived agent rows treat this pane
  // as a live PTY (buildTitleDerivedAgentRows gates on tabHasLivePty). Cleared
  // on dispose. `server:` prefix keeps it distinct from real local pty ids.
  let registeredPtyId: string | null = null
  const paneKey = makePaneKey(deps.tabId, pane.leafId)
  // Last title-derived state, so we can fire the task-complete notification on
  // the working→idle edge (the server path bypasses connectPanePty's hook).
  let lastTitleStatus: 'working' | 'permission' | 'idle' | null = null

  const agentTaskCompleteNotificationsEnabled = (): boolean => {
    const notifications = useAppStore.getState().settings?.notifications
    return notifications?.enabled !== false && notifications?.agentTaskComplete !== false
  }

  // Why: server-session bytes go straight to xterm and never touched the
  // agent-status pipeline, so the sidebar working/idle dot stayed blank for
  // every tmux-backed agent — and no "task complete" notification ever fired.
  // Route each OSC title into runtimePaneTitlesByTabId (what
  // buildTitleDerivedAgentRows reads) and raise the completion notification on
  // the working→idle transition, mirroring the local PTY path.
  const handleServerSessionTitle = (title: string): void => {
    if (disposed) {
      return
    }
    // Parity with the local path: cursor-agent's bare native title carries no
    // status and must not stomp a live working/idle state back to nothing.
    if (title.trim().toLowerCase() === 'cursor agent') {
      return
    }
    deps.setRuntimePaneTitle(deps.tabId, pane.id, title)
    if (manager.getActivePane()?.id === pane.id) {
      deps.updateTabTitle(deps.tabId, title)
    }
    const status = detectAgentStatusFromTitle(title)
    if (status === 'working' || status === 'idle' || status === 'permission') {
      if (
        lastTitleStatus === 'working' &&
        status === 'idle' &&
        agentTaskCompleteNotificationsEnabled()
      ) {
        deps.dispatchNotification({
          source: 'agent-task-complete',
          terminalTitle: title,
          paneKey
        })
      }
      lastTitleStatus = status
    }
  }

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
    // The desktop launches agents by typing a command into a shell (e.g.
    // `claude`). For a shell session, forward that startup command so the agent
    // actually attaches. For an agent-tool session the server launches it, so
    // sending the command again would double-launch — skip it.
    const startupCommand = tool === 'terminal' ? deps.startup?.command : undefined
    void (async () => {
      try {
        const session = await ensureWorkspaceSession({ workdir, tool })
        if (disposed) {
          return
        }
        binding = await bindServerSessionTerminal(session.id, pane.terminal, {
          startupCommand,
          onTitle: handleServerSessionTitle
        })
        if (disposed) {
          binding.dispose()
          binding = null
          return
        }
        // Mark the tab as having a live PTY so title-derived agent rows render.
        registeredPtyId = `server:${session.id}:${pane.leafId}`
        deps.updateTabPtyId(deps.tabId, registeredPtyId)
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
      if (registeredPtyId) {
        deps.clearTabPtyId(deps.tabId, registeredPtyId)
        deps.clearRuntimePaneTitle(deps.tabId, pane.id)
        registeredPtyId = null
      }
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
