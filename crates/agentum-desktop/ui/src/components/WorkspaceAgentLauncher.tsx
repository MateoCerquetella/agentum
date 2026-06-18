import React, { useCallback, useMemo } from 'react'
import { Terminal as TerminalIcon } from 'lucide-react'
import { toast } from 'sonner'
import { useAppStore } from '@/store'
import { AGENT_CATALOG, AgentIcon } from '@/lib/agent-catalog'
import { useDetectedAgents } from '@/hooks/useDetectedAgents'
import { launchAgentInNewTab } from '@/lib/launch-agent-in-new-tab'
import { filterEnabledTuiAgents } from '../../../shared/tui-agent-selection'
import type { TuiAgent } from '../../../shared/types'

/**
 * Empty-state for an active workspace that has no open session. Replaces the
 * old behaviour of auto-spawning a blank terminal the instant a workspace was
 * activated — the user asked to pick which agent to start *before* anything
 * opens. Rendered as `absolute inset-0` and self-centred (mirrors Landing) so
 * it always respects the window viewport instead of growing past it.
 */
export default function WorkspaceAgentLauncher({
  worktreeId
}: {
  worktreeId: string
}): React.JSX.Element {
  // Reactive connectionId (null = local) so the detected-agent list reflects the
  // right host — SSH worktrees probe agents on the remote.
  const connectionId = useAppStore((s) => {
    const worktree = Object.values(s.worktreesByRepo ?? {})
      .flat()
      .find((w) => w.id === worktreeId)
    if (!worktree) {
      return undefined
    }
    return s.repos?.find((r) => r.id === worktree.repoId)?.connectionId ?? null
  })
  const { detectedIds } = useDetectedAgents(connectionId)
  const disabledAgents = useAppStore((s) => s.settings?.disabledTuiAgents) ?? []
  const createTab = useAppStore((s) => s.createTab)
  const setActiveTabType = useAppStore((s) => s.setActiveTabType)

  // Catalog order, filtered to enabled agents and (when detection has resolved)
  // to those actually installed on this host. `null` detection = still probing,
  // so show all enabled rather than block the user behind a slow SSH probe.
  const agents = useMemo(() => {
    const enabled = new Set(
      filterEnabledTuiAgents(
        AGENT_CATALOG.map((a) => a.id),
        disabledAgents
      )
    )
    return AGENT_CATALOG.filter(
      (a) => enabled.has(a.id) && (detectedIds === null || detectedIds.includes(a.id))
    )
  }, [detectedIds, disabledAgents])

  const launch = useCallback(
    (agent: TuiAgent) => {
      const result = launchAgentInNewTab({ agent, worktreeId, launchSource: 'sidebar' })
      if (!result) {
        toast.error(`Could not start ${agent}.`)
      }
    },
    [worktreeId]
  )

  const openBlankTerminal = useCallback(() => {
    createTab(worktreeId)
    setActiveTabType('terminal')
  }, [createTab, setActiveTabType, worktreeId])

  return (
    <div className="absolute inset-0 flex items-center justify-center overflow-auto bg-background">
      <div className="w-full max-w-md px-6 py-8">
        <div className="mb-5 text-center">
          <h2 className="text-lg font-semibold text-foreground">Start a session</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            {agents.length > 0
              ? 'Choose an agent to launch in this workspace.'
              : 'No agents detected on this host.'}
          </p>
        </div>

        {agents.length > 0 ? (
          <div className="grid grid-cols-2 gap-2">
            {agents.map((agent) => (
              <button
                key={agent.id}
                type="button"
                onClick={() => launch(agent.id)}
                className="flex items-center gap-2 rounded-md border border-border/80 bg-secondary/40 px-3 py-2.5 text-sm text-foreground transition-colors hover:bg-accent cursor-pointer"
              >
                <span className="grid size-5 shrink-0 place-items-center">
                  <AgentIcon agent={agent.id} size={16} />
                </span>
                <span className="truncate">{agent.label}</span>
              </button>
            ))}
          </div>
        ) : null}

        <div className="mt-3 flex justify-center">
          <button
            type="button"
            onClick={openBlankTerminal}
            className="inline-flex items-center gap-1.5 rounded px-2 py-1 text-xs text-muted-foreground transition-colors hover:text-foreground cursor-pointer"
          >
            <TerminalIcon className="size-3.5" />
            Open a blank terminal
          </button>
        </div>
      </div>
    </div>
  )
}
