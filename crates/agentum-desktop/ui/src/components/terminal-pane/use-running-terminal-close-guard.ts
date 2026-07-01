import { useCallback, useState } from 'react'
import { toast } from 'sonner'
import { api } from '@/tauri'
import { useAppStore } from '@/store'
import { getConnectionId } from '@/lib/connection-context'
import { isRemoteRuntimePtyId } from '@/runtime/runtime-terminal-inspection'

type PendingClose = { proceed: () => void }

export type RunningTerminalCloseGuard = {
  /** Gate a terminal-tab close on a "Stop running command?" confirmation. If the
   *  tab's local PTYs have a running child (build, agent, …) and the user hasn't
   *  opted out, the dialog is shown and `proceed` fires only on confirm.
   *  Otherwise `proceed` runs immediately. */
  requestClose: (tabId: string, proceed: () => void) => void
  /** Spread onto <CloseTerminalDialog />. */
  dialog: {
    open: boolean
    dontAskAgain: boolean
    onDontAskAgainChange: (next: boolean) => void
    onCancel: () => void
    onConfirm: () => void
  }
}

/**
 * Shared "double-check before closing a session with a running command" guard.
 * Used by every terminal-tab close affordance (the group tab-strip X, the
 * fallback tab bar) so a stray close never silently kills an in-flight command.
 * Remote (SSH) sessions are skipped — they detach and persist through the relay,
 * so closing the tab doesn't stop the command.
 */
export function useRunningTerminalCloseGuard(): RunningTerminalCloseGuard {
  const [pending, setPending] = useState<PendingClose | null>(null)
  const [dontAskAgain, setDontAskAgain] = useState(false)

  const requestClose = useCallback((tabId: string, proceed: () => void) => {
    const state = useAppStore.getState()
    const owningWorktreeId =
      Object.entries(state.tabsByWorktree).find(([, tabs]) =>
        tabs.some((tab) => tab.id === tabId)
      )?.[0] ?? null
    // Remote sessions persist server-side; closing the tab won't kill the command.
    if (owningWorktreeId && getConnectionId(owningWorktreeId) != null) {
      proceed()
      return
    }
    if (state.settings?.skipRunningTerminalCloseConfirm) {
      proceed()
      return
    }
    const ptyIds = (state.ptyIdsByTabId[tabId] ?? []).filter((id) => !isRemoteRuntimePtyId(id))
    if (ptyIds.length === 0) {
      proceed()
      return
    }
    void Promise.all(ptyIds.map((id) => api.pty.hasChildProcesses(id)))
      .then((results) => {
        if (results.some(Boolean)) {
          setPending({ proceed })
        } else {
          proceed()
        }
      })
      // A wedged/missing probe shouldn't swallow the close — closing a tab that
      // might have had a child beats the X silently doing nothing. Matches the
      // per-pane Cmd+W guard's fallback semantics.
      .catch(() => proceed())
  }, [])

  const reset = useCallback(() => {
    setPending(null)
    setDontAskAgain(false)
  }, [])

  const onConfirm = useCallback(() => {
    if (!pending) {
      return
    }
    if (dontAskAgain) {
      void useAppStore.getState().updateSettings({ skipRunningTerminalCloseConfirm: true })
      toast.success("We'll close running terminals without asking next time.", {
        description: 'You can change this in Settings.',
        duration: 8000
      })
    }
    pending.proceed()
    reset()
  }, [pending, dontAskAgain, reset])

  return {
    requestClose,
    dialog: {
      open: pending !== null,
      dontAskAgain,
      onDontAskAgainChange: setDontAskAgain,
      onCancel: reset,
      onConfirm
    }
  }
}
