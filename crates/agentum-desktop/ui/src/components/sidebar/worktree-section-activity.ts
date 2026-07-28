import type { AppState } from '@/store/types'
import { resolveWorktreeStatus } from '@/lib/worktree-status'
import type {
  ProjectGroup,
  Repo,
  Worktree,
  WorkspaceStatusDefinition
} from '@/shared/types'
import {
  getGroupKeysForWorktree,
  type WorktreeGroupBy,
  PINNED_GROUP_KEY
} from './worktree-list-groups'
import {
  selectLivePtyIdsForWorktree,
  selectRuntimePaneTitlesForWorktree
} from './worktree-card-status-inputs'
import { selectWorktreeAgentActivitySummary } from './worktree-agent-activity-summary'
import { selectServerWorktreeActivity } from '@/store/slices/server-worktree-activity'

export type WorktreeSectionActivityState = Pick<
  AppState,
  | 'tabsByWorktree'
  | 'browserTabsByWorktree'
  | 'ptyIdsByTabId'
  | 'runtimePaneTitlesByTabId'
  | 'agentStatusEpoch'
  | 'agentStatusByPaneKey'
  | 'migrationUnsupportedByPtyId'
  | 'retainedAgentsByPaneKey'
  | 'awaitingInputByPaneKey'
  | 'serverWorktreeActivityByWorktreeId'
>

export type WorktreeSectionActivitySummary = {
  runningCount: number
  // Why: unread is the attention rollup. Unlike runningCount it survives the
  // working->idle transition (a finished agent leaves no live PTY), so a
  // collapsed project header can keep advertising "needs attention" until the
  // user opens the worktree and clearWorktreeUnread fires.
  unreadCount: number
}

export const EMPTY_WORKTREE_SECTION_ACTIVITY: WorktreeSectionActivitySummary = {
  runningCount: 0,
  unreadCount: 0
}

/**
 * Return `previous` when `next` carries identical per-group counts, so the
 * consumer can keep a stable Map identity. The summaries are rebuilt on every
 * agent-status epoch bump and passed as a prop to the memoized virtualized
 * viewport — without identity reuse, every agent transition re-rendered the
 * whole sidebar list even when no section count actually changed.
 */
export function reuseSectionActivitySummariesIfEqual(
  previous: Map<string, WorktreeSectionActivitySummary> | null,
  next: Map<string, WorktreeSectionActivitySummary>
): Map<string, WorktreeSectionActivitySummary> {
  if (!previous || previous.size !== next.size) {
    return next
  }
  for (const [groupKey, summary] of next) {
    const prev = previous.get(groupKey)
    if (
      !prev ||
      prev.runningCount !== summary.runningCount ||
      prev.unreadCount !== summary.unreadCount
    ) {
      return next
    }
  }
  return previous
}

export function buildWorktreeSectionActivitySummaries({
  groupBy,
  worktrees,
  repoMap,
  prCache,
  workspaceStatuses,
  settings,
  projectGroups,
  state
}: {
  groupBy: WorktreeGroupBy
  worktrees: readonly Worktree[]
  repoMap: Map<string, Repo>
  prCache: Record<string, unknown> | null
  workspaceStatuses: readonly WorkspaceStatusDefinition[]
  settings?: AppState['settings']
  projectGroups: readonly ProjectGroup[]
  state: WorktreeSectionActivityState
}): Map<string, WorktreeSectionActivitySummary> {
  const summaries = new Map<string, WorktreeSectionActivitySummary>()

  for (const worktree of worktrees) {
    const groupKeys = worktree.isPinned
      ? [PINNED_GROUP_KEY]
      : getGroupKeysForWorktree(
          groupBy,
          worktree,
          repoMap,
          prCache,
          workspaceStatuses,
          settings,
          projectGroups
        )
    if (groupKeys.length === 0) {
      continue
    }

    const status = resolveWorktreeStatusFromState(state, worktree.id)
    for (const groupKey of groupKeys) {
      const summary = summaries.get(groupKey) ?? { ...EMPTY_WORKTREE_SECTION_ACTIVITY }
      if (status === 'working') {
        summary.runningCount++
      }
      if (worktree.isUnread) {
        summary.unreadCount++
      }
      summaries.set(groupKey, summary)
    }
  }

  return summaries
}

export function resolveWorktreeStatusFromState(
  state: WorktreeSectionActivityState,
  worktreeId: string,
  now?: number
): ReturnType<typeof resolveWorktreeStatus> {
  const agentSummary = selectWorktreeAgentActivitySummary(state, worktreeId, now)
  const serverActivity = selectServerWorktreeActivity(state, worktreeId)

  // Why: collapsed headers must mirror the card dot semantics exactly; otherwise
  // a hidden section can advertise different activity than its visible cards.
  return resolveWorktreeStatus({
    tabs: state.tabsByWorktree[worktreeId] ?? [],
    browserTabs: state.browserTabsByWorktree[worktreeId] ?? [],
    ptyIdsByTabId: selectLivePtyIdsForWorktree(state, worktreeId),
    runtimePaneTitlesByTabId: selectRuntimePaneTitlesForWorktree(state, worktreeId),
    hasPermission: agentSummary.hasPermission,
    hasLiveWorking: agentSummary.hasLiveWorking,
    hasLiveDone: agentSummary.hasLiveDone,
    hasRetainedDone: agentSummary.hasRetainedDone,
    isAlive: serverActivity.isAlive,
    liveActivity: serverActivity.liveActivity
  })
}
