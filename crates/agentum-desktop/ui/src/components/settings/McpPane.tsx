import { useEffect, useState } from 'react'
import { Label } from '../ui/label'
import { getMcpSettings, setMcpSettings } from '@/runtime/agentum-server-client'
import { SearchableSetting } from './SearchableSetting'
import { matchesSettingsSearch } from './settings-search'
import { useAppStore } from '../../store'
import { MCP_PANE_SEARCH_ENTRIES } from './mcp-search'

export function McpPane(): React.JSX.Element {
  const searchQuery = useAppStore((s) => s.settingsSearchQuery)
  const showMcp = matchesSettingsSearch(searchQuery, MCP_PANE_SEARCH_ENTRIES)

  // The real gate is the server-side `mcp.enabled` setting (agentum-server
  // `routes/mcp.rs`), read at agent-launch time by `mcp_provision::provision`.
  // Default ON; reconcile with the server after mount.
  const [enabled, setEnabled] = useState<boolean>(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void getMcpSettings()
      .then((s) => {
        if (!cancelled) setEnabled(s.enabled)
      })
      .catch(() => {
        // Leave the optimistic default on; the toggle still works and surfaces a
        // write error if the server is unreachable.
      })
    return () => {
      cancelled = true
    }
  }, [])

  const toggleMcp = (value: boolean): void => {
    // Optimistic: flip the UI, write the server flag, revert if it fails.
    setEnabled(value)
    setError(null)
    void setMcpSettings(value).catch((err: unknown) => {
      setEnabled(!value)
      setError(err instanceof Error ? err.message : 'Could not update the Agent MCP setting.')
    })
  }

  if (!showMcp) {
    return <div />
  }

  return (
    <SearchableSetting
      title="Agent MCP"
      description="Master switch for agentum's built-in MCP tools that every agent agentum launches can call."
      keywords={MCP_PANE_SEARCH_ENTRIES[0].keywords}
      className="space-y-3 py-2"
    >
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 shrink space-y-0.5">
          <Label>Agent MCP</Label>
          <p className="text-xs text-muted-foreground">
            agentum wires its own MCP server into every agent it launches, giving them tools to
            manage sessions &amp; worktrees, drive the browser and computer, run the harness, and
            orchestrate other agents. Turn this off to wire <strong>no</strong> agentum tools into
            any agent — the per-capability toggles below have no effect while it&apos;s off.
          </p>
        </div>
        <button
          role="switch"
          aria-checked={enabled}
          onClick={() => toggleMcp(!enabled)}
          className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent transition-colors ${
            enabled ? 'bg-foreground' : 'bg-muted-foreground/30'
          }`}
        >
          <span
            className={`inline-block h-3.5 w-3.5 transform rounded-full bg-background shadow-sm transition-transform ${
              enabled ? 'translate-x-4' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>

      {error ? <p className="text-xs text-destructive">{error}</p> : null}

      {!enabled ? (
        <p className="rounded-lg border border-border/60 p-3 text-xs text-muted-foreground">
          agentum&apos;s MCP is off: new agents launch with none of agentum&apos;s tools. Agents that
          are already running keep what they were launched with until restarted. Turn it back on to
          restore them.
        </p>
      ) : null}
    </SearchableSetting>
  )
}
