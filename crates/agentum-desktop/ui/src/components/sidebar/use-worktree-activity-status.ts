import { useMemo } from 'react'
import { useShallow } from 'zustand/react/shallow'
import { useAppStore } from '@/store'
import { resolveWorktreeStatus, type WorktreeStatus } from '@/lib/worktree-status'
import { EMPTY_BROWSER_TABS, EMPTY_TABS } from './WorktreeCardHelpers'
import {
  selectLivePtyIdsForWorktree,
  selectRuntimePaneTitlesForWorktree
} from './worktree-card-status-inputs'
import { selectWorktreeAgentActivitySummary } from './worktree-agent-activity-summary'
import { selectServerWorktreeActivity } from '@/store/slices/server-worktree-activity'

export function useWorktreeActivityStatus(worktreeId: string): WorktreeStatus {
  const tabs = useAppStore((s) => s.tabsByWorktree[worktreeId] ?? EMPTY_TABS)
  const browserTabs = useAppStore((s) => s.browserTabsByWorktree[worktreeId] ?? EMPTY_BROWSER_TABS)
  const runtimePaneTitlesForWorktree = useAppStore(
    useShallow((s) => selectRuntimePaneTitlesForWorktree(s, worktreeId))
  )
  const ptyIdsForWorktree = useAppStore(
    useShallow((s) => selectLivePtyIdsForWorktree(s, worktreeId))
  )
  const { hasPermission, hasLiveWorking, hasLiveDone, hasRetainedDone } = useAppStore(
    useShallow((s) => selectWorktreeAgentActivitySummary(s, worktreeId))
  )
  // Server-authoritative liveness + activity (from /api/sessions + /api/events),
  // so the dot reflects a running agent after relaunch even before its pane mounts.
  const { isAlive, liveActivity } = useAppStore(
    useShallow((s) => selectServerWorktreeActivity(s, worktreeId))
  )

  // Why: compact and detailed cards need the same status-dot semantics:
  // runtime liveness gates title-derived states, then explicit agent rows can
  // promote working/permission/done so the dot matches visible agent state.
  return useMemo(
    () =>
      resolveWorktreeStatus({
        tabs,
        browserTabs,
        ptyIdsByTabId: ptyIdsForWorktree,
        runtimePaneTitlesByTabId: runtimePaneTitlesForWorktree,
        hasPermission,
        hasLiveWorking,
        hasLiveDone,
        hasRetainedDone,
        isAlive,
        liveActivity
      }),
    [
      tabs,
      browserTabs,
      ptyIdsForWorktree,
      runtimePaneTitlesForWorktree,
      hasPermission,
      hasLiveWorking,
      hasLiveDone,
      hasRetainedDone,
      isAlive,
      liveActivity
    ]
  )
}
