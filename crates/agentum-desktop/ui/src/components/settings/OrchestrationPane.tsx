import { useEffect, useState } from 'react'
import { Label } from '../ui/label'
import {
  ORCHESTRATION_SETUP_STATE_EVENT,
  isOrchestrationSetupEnabled,
  persistOrchestrationEnabled,
  syncOrchestrationEnabledFromServer
} from '@/lib/orchestration-setup-state'
import { SearchableSetting } from './SearchableSetting'
import { matchesSettingsSearch } from './settings-search'
import { useAppStore } from '../../store'
import { ORCHESTRATION_PANE_SEARCH_ENTRIES } from './orchestration-search'

export function OrchestrationPane(): React.JSX.Element {
  const searchQuery = useAppStore((s) => s.settingsSearchQuery)
  const showOrchestration = matchesSettingsSearch(searchQuery, ORCHESTRATION_PANE_SEARCH_ENTRIES)

  // Paint synchronously from the cache, then reconcile with the server flag —
  // the real gate lives server-side (agentum-server `routes/mcp.rs`), not here.
  const [orchestrationEnabled, setOrchestrationEnabled] = useState<boolean>(() =>
    isOrchestrationSetupEnabled()
  )
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void syncOrchestrationEnabledFromServer().then((enabled) => {
      if (!cancelled) setOrchestrationEnabled(enabled)
    })
    const syncSetupState = (): void => {
      setOrchestrationEnabled(isOrchestrationSetupEnabled())
    }
    window.addEventListener(ORCHESTRATION_SETUP_STATE_EVENT, syncSetupState)
    return () => {
      cancelled = true
      window.removeEventListener(ORCHESTRATION_SETUP_STATE_EVENT, syncSetupState)
    }
  }, [])

  const toggleOrchestration = (value: boolean): void => {
    // Optimistic: flip the UI, write the server flag, revert if it fails.
    setOrchestrationEnabled(value)
    setError(null)
    if (value) {
      useAppStore.getState().recordFeatureInteraction('agent-orchestration-setup')
    }
    void persistOrchestrationEnabled(value).catch((err: unknown) => {
      setOrchestrationEnabled(!value)
      setError(err instanceof Error ? err.message : 'Could not update orchestration.')
    })
  }

  if (!showOrchestration) {
    return <div />
  }

  return (
    <SearchableSetting
      title="Agent Orchestration"
      description="Coordinate multiple coding agents via messaging, task DAGs, dispatch, and decision gates."
      keywords={ORCHESTRATION_PANE_SEARCH_ENTRIES[0].keywords}
      className="space-y-3 py-2"
    >
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 shrink space-y-0.5">
          <Label>Agent Orchestration</Label>
          <p className="text-xs text-muted-foreground">
            A built-in agentum MCP capability — no skill to install. When on, agentum&apos;s MCP
            server exposes the orchestration tools (inter-agent messaging and the task DAG) to every
            agent it launches, so agents can hand off work, coordinate, and dispatch tasks.
          </p>
        </div>
        <button
          role="switch"
          aria-checked={orchestrationEnabled}
          onClick={() => toggleOrchestration(!orchestrationEnabled)}
          className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent transition-colors ${
            orchestrationEnabled ? 'bg-foreground' : 'bg-muted-foreground/30'
          }`}
        >
          <span
            className={`inline-block h-3.5 w-3.5 transform rounded-full bg-background shadow-sm transition-transform ${
              orchestrationEnabled ? 'translate-x-4' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>

      {error ? <p className="text-xs text-destructive">{error}</p> : null}

      {orchestrationEnabled ? (
        <p className="rounded-lg border border-border/60 p-3 text-xs text-muted-foreground">
          Orchestration tools are live: agents launched by agentum can call{' '}
          <code className="text-foreground">agentum_send_message</code>,{' '}
          <code className="text-foreground">agentum_check_messages</code>, and the task-DAG tools
          over the MCP. Turn this off to remove them from every agent.
        </p>
      ) : null}
    </SearchableSetting>
  )
}
