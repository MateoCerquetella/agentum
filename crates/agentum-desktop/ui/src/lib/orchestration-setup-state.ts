import {
  getOrchestrationSettings,
  setOrchestrationSettings
} from '@/runtime/agentum-server-client'

export const ORCHESTRATION_SETUP_STATE_EVENT = 'agentum:orchestration-setup-state'
export const ORCHESTRATION_ENABLED_STORAGE_KEY = 'agentum.orchestration.enabled'
export const ORCHESTRATION_SETUP_DISMISSED_STORAGE_KEY = 'agentum.orchestration.setupDismissed'

// Orchestration is an agentum MCP capability (inter-agent mailbox + task DAG),
// not an installable skill. Its real on/off switch lives server-side
// (`/api/orchestration/settings`, gated in agentum-server `routes/mcp.rs`). The
// localStorage key below is only a *cache* of that flag so the toggle can paint
// synchronously on mount; `syncOrchestrationEnabledFromServer()` reconciles it
// with the server (and migrates a pre-MCP localStorage value up to the server).

export function isOrchestrationSetupEnabled(): boolean {
  return localStorage.getItem(ORCHESTRATION_ENABLED_STORAGE_KEY) === '1'
}

export function hasOrchestrationSetupMarker(): boolean {
  return localStorage.getItem(ORCHESTRATION_ENABLED_STORAGE_KEY) !== null
}

export function isOrchestrationSetupDismissed(): boolean {
  return localStorage.getItem(ORCHESTRATION_SETUP_DISMISSED_STORAGE_KEY) === '1'
}

export function notifyOrchestrationSetupStateChanged(): void {
  window.dispatchEvent(new CustomEvent(ORCHESTRATION_SETUP_STATE_EVENT))
}

/** Mirror a value into the localStorage cache (no server write, no event). */
function writeCache(enabled: boolean): void {
  localStorage.setItem(ORCHESTRATION_ENABLED_STORAGE_KEY, enabled ? '1' : '0')
}

/**
 * Read the authoritative server flag, reconcile the local cache, and return it.
 *
 * Migration: a user who enabled orchestration in the pre-MCP build has the
 * cache set to `'1'` but never wrote the server flag. When the server reports
 * `false` while the cache says enabled, push the cache value up once so their
 * choice survives the move to the server-side gate. On any error, fall back to
 * the cached value so the toggle still renders something sane offline.
 */
export async function syncOrchestrationEnabledFromServer(): Promise<boolean> {
  try {
    const { enabled } = await getOrchestrationSettings()
    if (!enabled && isOrchestrationSetupEnabled()) {
      await setOrchestrationSettings(true)
      writeCache(true)
      notifyOrchestrationSetupStateChanged()
      return true
    }
    writeCache(enabled)
    return enabled
  } catch {
    return isOrchestrationSetupEnabled()
  }
}

/**
 * Set the server flag (the real gate), then update the cache and notify
 * listeners. Throws if the server write fails so callers can surface it.
 */
export async function persistOrchestrationEnabled(enabled: boolean): Promise<void> {
  await setOrchestrationSettings(enabled)
  writeCache(enabled)
  if (enabled) {
    localStorage.removeItem(ORCHESTRATION_SETUP_DISMISSED_STORAGE_KEY)
  }
  notifyOrchestrationSetupStateChanged()
}
