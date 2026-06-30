import { toast } from 'sonner'
import { resolveServerSessionId, uploadLocalImageToSession } from './screenshot-remote-upload'
import { getConnectionId } from '@/lib/connection-context'
import { extractIpcErrorMessage } from '@/lib/ipc-error'
import type { ManagedPane, PaneManager } from '@/lib/pane-manager/pane-manager'
import { useAppStore } from '@/store'
import { isWindowsUserAgent, shellEscapePath } from './pane-helpers'
import type { PtyTransport } from './pty-transport'
import { importExternalPathsToRuntime } from '@/runtime/runtime-file-client'
import { isWindowsAbsolutePathLike } from '../../../../shared/cross-platform-path'

type Args = {
  manager: PaneManager
  paneTransports: Map<number, PtyTransport>
  worktreeId: string
  cwd: string | undefined
  /** The dropped-on tab. Needed to resolve the server session id for an SSH
   *  worktree, whose agent terminal is a server-session pane (no PtyTransport). */
  tabId: string
  data: { paths: string[]; target: string; tabId?: string }
}

export type TerminalTargetShell = 'posix' | 'windows'

export function getTerminalTargetShellForWorktreePath(worktreePath: string): TerminalTargetShell {
  return isWindowsPathLike(worktreePath) ? 'windows' : 'posix'
}

export function resolveTerminalDropTargetShell({
  activeRuntimeEnvironmentId,
  worktreePath,
  connectionId,
  userAgent
}: {
  activeRuntimeEnvironmentId: string | null | undefined
  worktreePath: string | null | undefined
  connectionId: string | null | undefined
  userAgent?: string
}): TerminalTargetShell {
  if (activeRuntimeEnvironmentId?.trim() && worktreePath) {
    return getTerminalTargetShellForWorktreePath(worktreePath)
  }
  if (typeof connectionId === 'string') {
    return 'posix'
  }
  return isWindowsUserAgent(userAgent) ? 'windows' : 'posix'
}

/**
 * Inject a (shell-escaped) path into a terminal pane's input. Local-PTY panes
 * carry a `PtyTransport`; agentum's AGENT terminals are server-session panes
 * that are NOT in `paneTransports` and route input through xterm `onData →
 * stream.send`, reached via `terminal.paste`. The drop handler used to require a
 * transport and bail without one, so every drop onto an agent terminal was
 * silently discarded. Falling back to `terminal.paste` (the same inject the
 * working Cmd+V image-paste uses) is what makes drag-drop reach agent terminals.
 */
function deliverPathToPane(
  pane: ManagedPane,
  paneTransports: Map<number, PtyTransport>,
  text: string
): void {
  const transport = paneTransports.get(pane.id)
  if (transport) {
    transport.sendInput(text)
  } else {
    pane.terminal.paste(text)
  }
}

/**
 * Handle a native file drop targeted at a terminal pane.
 *
 * Local worktrees: paste the local absolute path (reference-in-place; no copy
 * or IPC). SSH worktrees: upload each file into `${worktreePath}/.agentum/drops`
 * and paste the remote path so the remote agent can read it. See
 * docs/terminal-drop-ssh.md.
 */
export async function handleTerminalFileDrop(args: Args): Promise<void> {
  const { manager, paneTransports, worktreeId, cwd, tabId, data } = args
  if (data.paths.length === 0) {
    return
  }
  const pane = manager.getActivePane() ?? manager.getPanes()[0]
  if (!pane) {
    return
  }
  const paneId = pane.id
  const settings = useAppStore.getState().settings
  const activeRuntimeEnvironmentId = settings?.activeRuntimeEnvironmentId?.trim()
  const worktreePath = resolveWorktreePath(worktreeId, cwd)
  if (!worktreePath) {
    toast.error('Worktree path not available.')
    return
  }

  if (activeRuntimeEnvironmentId) {
    const targetShell = getTerminalTargetShellForWorktreePath(worktreePath)
    const destinationDir = joinRuntimeDropDir(worktreePath)
    const pending = toast.loading(
      `Uploading ${data.paths.length} file${data.paths.length === 1 ? '' : 's'} to runtime…`
    )
    try {
      const { results } = await importExternalPathsToRuntime(
        {
          settings,
          worktreeId,
          worktreePath
        },
        data.paths,
        destinationDir
      )
      const imported = results.filter((result) => result.status === 'imported')
      const skipped = results.filter((result) => result.status === 'skipped')
      const failed = results.filter((result) => result.status === 'failed')
      const livePane = manager.getPanes().find((p) => p.id === paneId)
      if (livePane) {
        for (const result of imported) {
          const shellPath = isWindowsPathLike(worktreePath)
            ? result.destPath.replace(/\//g, '\\')
            : result.destPath
          deliverPathToPane(livePane, paneTransports, `${shellEscapePath(shellPath, targetShell)} `)
        }
        livePane.terminal.focus()
      }
      reportUploadSkipsAndFailures(skipped, failed)
    } catch (err) {
      toast.error(extractIpcErrorMessage(err, 'Failed to upload files.'))
    } finally {
      toast.dismiss(pending)
    }
    return
  }

  // Why: `getConnectionId` returns `string` (SSH), `null` (local repo found),
  // or `undefined` (store not hydrated / worktree not found). Treat
  // `undefined` as an error — otherwise a drop during hydration would
  // silently paste local paths into a remote shell.
  const connectionId = getConnectionId(worktreeId)
  if (connectionId === undefined) {
    toast.error('Worktree not ready — try again in a moment.')
    return
  }
  const isRemote = connectionId !== null
  const targetShell = resolveTerminalDropTargetShell({
    activeRuntimeEnvironmentId: null,
    worktreePath,
    connectionId
  })

  // Why: local fast path — no IPC round-trip, no toast — preserves today's
  // zero-latency drop behavior. Trailing space separates multiple paths in
  // the terminal input, matching standard drag-and-drop UX conventions.
  if (!isRemote) {
    for (const p of data.paths) {
      deliverPathToPane(pane, paneTransports, `${shellEscapePath(p, targetShell)} `)
    }
    pane.terminal.focus()
    return
  }

  // SSH worktree: the agent runs on a remote host, so a dragged-in LOCAL path is
  // unreachable to it. (The previous SFTP-style command was an unimplemented stub
  // that returned no paths — nothing ever landed.) Read each dropped file's bytes
  // and POST them to the host-aware uploads route, which writes them onto the
  // remote host and types the path into the remote pane. The server injects, so
  // we don't client-paste here.
  const sessionId = resolveServerSessionId(tabId, pane.leafId)
  if (!sessionId) {
    toast.error('No agent session for this worktree.')
    return
  }
  const pending = toast.loading(
    `Uploading ${data.paths.length} file${data.paths.length === 1 ? '' : 's'} to remote…`
  )
  try {
    for (const p of data.paths) {
      await uploadLocalImageToSession(sessionId, p)
    }
    // Re-check the pane survived the async upload (tab closed / worktree
    // switched) before focusing it.
    manager.getPanes().find((pn) => pn.id === paneId)?.terminal.focus()
  } catch (err) {
    toast.error(extractIpcErrorMessage(err, 'Failed to upload files.'))
  } finally {
    toast.dismiss(pending)
  }
}

function reportUploadSkipsAndFailures(
  skipped: { reason: string }[],
  failed: { reason: string }[]
): void {
  if (skipped.length > 0) {
    // Why: symlink rejection is policy, not error — show as neutral
    // message. Mixed skips collapse to a single "items" count to avoid
    // enumerating every reason.
    const symlinkCount = skipped.filter((s) => s.reason === 'symlink').length
    const noun = skipped.length === 1 ? 'item' : 'items'
    toast.message(
      symlinkCount === skipped.length
        ? `Skipped ${skipped.length} symlink${skipped.length === 1 ? '' : 's'}.`
        : `Skipped ${skipped.length} ${noun}.`
    )
  }
  if (failed.length > 0) {
    const noun = failed.length === 1 ? 'file' : 'files'
    toast.error(`Failed to upload ${failed.length} ${noun}.`)
  }
}

function resolveWorktreePath(worktreeId: string, fallbackCwd: string | undefined): string | null {
  const state = useAppStore.getState()
  const allWorktrees = Object.values(state.worktreesByRepo ?? {}).flat()
  const worktree = allWorktrees.find((w) => w.id === worktreeId)
  return worktree?.path ?? fallbackCwd ?? null
}

function joinRuntimeDropDir(worktreePath: string): string {
  if (isWindowsPathLike(worktreePath)) {
    return `${worktreePath.replace(/[\\/]+$/, '').replace(/\//g, '\\')}\\.agentum\\drops`
  }
  return `${worktreePath.replace(/[\\/]+$/, '')}/.agentum/drops`
}

function isWindowsPathLike(path: string): boolean {
  return isWindowsAbsolutePathLike(path) || path.includes('\\')
}
