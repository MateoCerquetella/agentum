import type { AgentStatusEntry } from '@/shared/agent-status-types'
import { selectLiveAgentStatusEntriesForWorktree } from './worktree-agent-row-selectors'

/** The fields the active-session card + leaf ctx% chip render. All optional —
 *  agents report tool/message/context independently. */
export type LatestAgentActivity = {
  lastAssistantMessage?: string
  toolName?: string
  toolInput?: string
  contextUsagePercent?: number
}

const EMPTY: LatestAgentActivity = {}

/** Pick the most-recently-updated agent entry's surface fields. Pure over an
 *  entry array so it's trivially unit-testable and reusable from the hook. */
export function latestFromEntries(entries: readonly AgentStatusEntry[]): LatestAgentActivity {
  let latest: AgentStatusEntry | undefined
  for (const entry of entries) {
    if (!latest || entry.updatedAt > latest.updatedAt) {
      latest = entry
    }
  }
  if (!latest) {
    return EMPTY
  }
  return {
    lastAssistantMessage: latest.lastAssistantMessage,
    toolName: latest.toolName,
    toolInput: latest.toolInput,
    contextUsagePercent: latest.contextUsagePercent
  }
}

function selectLatestAgentActivity(
  state: Parameters<typeof selectLiveAgentStatusEntriesForWorktree>[0],
  worktreeId: string
): LatestAgentActivity {
  return latestFromEntries(selectLiveAgentStatusEntriesForWorktree(state, worktreeId))
}
