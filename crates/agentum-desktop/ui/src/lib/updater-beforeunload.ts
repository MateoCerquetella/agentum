import {
  AGENTUM_APP_RESTART_ABORTED_EVENT,
  AGENTUM_APP_RESTART_STARTED_EVENT,
  AGENTUM_UPDATER_QUIT_AND_INSTALL_ABORTED_EVENT,
  AGENTUM_UPDATER_QUIT_AND_INSTALL_STARTED_EVENT
} from '@/shared/updater-renderer-events'

let intentionalAppRestartInProgress = false

export function isUpdaterQuitAndInstallInProgress(): boolean {
  return isIntentionalAppRestartInProgress()
}

export function isIntentionalAppRestartInProgress(): boolean {
  return intentionalAppRestartInProgress
}

export function registerUpdaterBeforeUnloadBypass(): () => void {
  const markInProgress = (): void => {
    intentionalAppRestartInProgress = true
  }
  const clearInProgress = (): void => {
    intentionalAppRestartInProgress = false
  }

  window.addEventListener(AGENTUM_UPDATER_QUIT_AND_INSTALL_STARTED_EVENT, markInProgress)
  window.addEventListener(AGENTUM_UPDATER_QUIT_AND_INSTALL_ABORTED_EVENT, clearInProgress)
  window.addEventListener(AGENTUM_APP_RESTART_STARTED_EVENT, markInProgress)
  window.addEventListener(AGENTUM_APP_RESTART_ABORTED_EVENT, clearInProgress)

  return () => {
    window.removeEventListener(AGENTUM_UPDATER_QUIT_AND_INSTALL_STARTED_EVENT, markInProgress)
    window.removeEventListener(AGENTUM_UPDATER_QUIT_AND_INSTALL_ABORTED_EVENT, clearInProgress)
    window.removeEventListener(AGENTUM_APP_RESTART_STARTED_EVENT, markInProgress)
    window.removeEventListener(AGENTUM_APP_RESTART_ABORTED_EVENT, clearInProgress)
    // Why: hot reloads can re-register this listener inside the same renderer.
    // Reset the module flag on cleanup so a failed earlier restart attempt
    // cannot silently suppress future unsaved-change prompts.
    intentionalAppRestartInProgress = false
  }
}
