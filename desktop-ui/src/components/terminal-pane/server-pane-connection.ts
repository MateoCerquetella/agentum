// Additive, opt-in Option A path: connect a terminal pane to a server session
// (a tmux pane streamed over WS by the embedded agentum-server) instead of a
// local PTY. Mirrors connectPanePty's PanePtyBinding contract so the pane
// lifecycle treats both identically. Off by default — see shouldUseServerTerminals.
import type { PaneManager, ManagedPane } from '@/lib/pane-manager/pane-manager'
import type { PanePtyBinding } from './pty-connection'
import type { PtyConnectionDeps } from './pty-connection-types'
import { ensureWorkspaceSession } from '@/runtime/workspace-session'
import {
  bindServerSessionTerminal,
  type ServerSessionTerminalBinding
} from '@/runtime/server-session-terminal'

/**
 * Off by default. Flip `localStorage['agentum.serverTerminals'] = '1'` (and
 * reload) to route new terminals through the embedded server's tmux sessions —
 * the Option A path that lets SSH/remote sessions survive disconnection.
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
 * this pane's workspace (workdir) and bind its tmux pane to the xterm. The
 * PTY-specific deps (restored pty ids, etc.) are intentionally ignored — the
 * server owns pane lifecycle, persistence, and reattach.
 */
export function connectPaneServerSession(
  pane: ManagedPane,
  _manager: PaneManager,
  deps: PtyConnectionDeps
): PanePtyBinding {
  let disposed = false
  let binding: ServerSessionTerminalBinding | null = null
  const workdir = deps.cwd ?? ''

  if (!workdir) {
    pane.terminal.write('\r\n\x1b[31m[agentum: no workdir for server session]\x1b[0m\r\n')
  } else {
    void (async () => {
      try {
        // tool 'terminal' = a plain shell pane; agent panes can pass their tool later.
        const session = await ensureWorkspaceSession({ workdir, tool: 'terminal' })
        if (disposed) {
          return
        }
        binding = await bindServerSessionTerminal(session.id, pane.terminal)
        if (disposed) {
          binding.dispose()
          binding = null
        }
      } catch (error) {
        pane.terminal.write(
          `\r\n\x1b[31m[agentum: server session failed: ${String(error)}]\x1b[0m\r\n`
        )
      }
    })()
  }

  return {
    dispose: () => {
      disposed = true
      binding?.dispose()
      binding = null
    },
    // The server owns the pane; renderer-local visibility/process tracking that
    // the local-PTY path needs are no-ops here.
    syncRendererOutputVisibility: () => {},
    syncProcessTracking: () => {}
  }
}
