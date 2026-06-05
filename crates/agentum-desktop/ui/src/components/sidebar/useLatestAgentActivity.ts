import { useMemo } from 'react'
import { useShallow } from 'zustand/react/shallow'
import { useAppStore } from '@/store'
import { selectLiveAgentStatusEntriesForWorktree } from './worktree-agent-row-selectors'
import { latestFromEntries, type LatestAgentActivity } from './worktree-latest-activity'

/**
 * Last assistant message + tool call + context% for a worktree's most-recently
 * active agent pane. Narrows the subscription to THIS worktree's entries via
 * useShallow (same render-amplification guard as useWorktreeAgentRows), and
 * threads agentStatusEpoch so freshness boundaries recompute.
 */
export function useLatestAgentActivity(worktreeId: string): LatestAgentActivity {
  const entries = useAppStore(
    useShallow((s) => selectLiveAgentStatusEntriesForWorktree(s, worktreeId))
  )
  const agentStatusEpoch = useAppStore((s) => s.agentStatusEpoch)
  return useMemo(
    () => latestFromEntries(entries),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [entries, agentStatusEpoch]
  )
}
