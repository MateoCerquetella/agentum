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
  // A tab launched with an agent keeps a live agent session even when its idle
  // title is unrecognizable (codex's idle title is just the cwd basename), so
  // treat an unknown title as idle rather than dropping the status entirely.
  const isAgentTab = resolveSessionTool(deps) !== 'terminal'
  // Last COMMITTED status — drives the working→idle completion notification and
  // the spinner-flicker debounce below.
  let committedTitleStatus: 'working' | 'permission' | 'idle' | null = null
  let idleHoldTimer: ReturnType<typeof setTimeout> | null = null
  let pendingIdleTitle: string | null = null
  // Why: codex animates its spinner by interleaving a bare cwd title between
  // braille frames mid-turn. A working→idle edge must persist for this window
  // before it counts as a real turn end — otherwise every flicker fires a false
  // completion notification and blinks the sidebar dot. Mirrors the local PTY
  // path's WORKING_TITLE_HOLD_MS.
  const WORKING_TO_IDLE_HOLD_MS = 700

  const clearIdleHold = (): void => {
    if (idleHoldTimer) {
      clearTimeout(idleHoldTimer)
      idleHoldTimer = null
    }
    pendingIdleTitle = null
  }

  const agentTaskCompleteNotificationsEnabled = (): boolean => {
    const notifications = useAppStore.getState().settings?.notifications
    return notifications?.enabled !== false && notifications?.agentTaskComplete !== false
  }

  const commitServerSessionStatus = (
    title: string,
    status: 'working' | 'permission' | 'idle'
  ): void => {
    deps.setRuntimePaneTitle(deps.tabId, pane.id, title)
    if (manager.getActivePane()?.id === pane.id) {
      deps.updateTabTitle(deps.tabId, title)
    }
    const justFinished = committedTitleStatus === 'working' && status === 'idle'
    if (justFinished && agentTaskCompleteNotificationsEnabled()) {
      deps.dispatchNotification({ source: 'agent-task-complete', terminalTitle: title, paneKey })
    }
    // Green ✓ "done" on a real turn end; cleared the moment the agent works
    // again (or is torn down, in dispose). A fresh idle that never worked stays
    // grey because justFinished is only true on a working→idle edge.
    const store = useAppStore.getState()
    if (justFinished) {
      store.markServerAgentDone(paneKey)
    } else if (status === 'working' || status === 'permission') {
      store.clearServerAgentDone(paneKey)
    }
    committedTitleStatus = status
  }

  // Why: server-session bytes go straight to xterm and never touched the
  // agent-status pipeline, so the sidebar dot stayed blank and no "task
  // complete" notification ever fired for tmux-backed agents. Route each OSC
  // title into runtimePaneTitlesByTabId (what buildTitleDerivedAgentRows reads),
  // map a known agent's unrecognized title to idle (so the row survives a turn
  // end), and raise the completion notification on a SUSTAINED working→idle.
  const handleServerSessionTitle = (title: string): void => {
    if (disposed) {
      return
    }
    // Parity with the local path: cursor-agent's bare native title carries no
    // status and must not stomp a live working/idle state back to nothing.
    if (title.trim().toLowerCase() === 'cursor agent') {
      return
    }
    const status: 'working' | 'permission' | 'idle' | null =
      detectAgentStatusFromTitle(title) ?? (isAgentTab ? 'idle' : null)
    if (status === null) {
      // Plain shell line on a non-agent tab — reflect it with no status meaning.
      deps.setRuntimePaneTitle(deps.tabId, pane.id, title)
      if (manager.getActivePane()?.id === pane.id) {
        deps.updateTabTitle(deps.tabId, title)
      }
      return
    }
    if (status === 'working' || status === 'permission') {
      // A live frame wins immediately and cancels a pending completion, so
      // codex's bare-title flicker can never read as a finished turn.
      clearIdleHold()
      commitServerSessionStatus(title, status)
      return
    }
    // status === 'idle': hold a working→idle edge briefly; a returning working
    // frame cancels it. Only a sustained idle commits and notifies.
    if (committedTitleStatus === 'working') {
      pendingIdleTitle = title
      if (!idleHoldTimer) {
        idleHoldTimer = setTimeout(() => {
          idleHoldTimer = null
          const heldTitle = pendingIdleTitle ?? title
          pendingIdleTitle = null
          if (!disposed) {
            commitServerSessionStatus(heldTitle, 'idle')
          }
        }, WORKING_TO_IDLE_HOLD_MS)
      }
      return
    }
    commitServerSessionStatus(title, status)
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
      clearIdleHold()
      useAppStore.getState().clearServerAgentDone(paneKey)
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
