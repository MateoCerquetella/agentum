import type { WorkspaceStatus, WorkspaceStatusDefinition, Worktree } from '../../../../shared/types'
import type { SortBy } from './smart-sort'
import type { TrackerPhaseWire } from '../../lib/tracker-phase'
import {
  WORKSPACE_KANBAN_TRACKER_LANES,
  getWorkspaceKanbanTrackerLane
} from './workspace-kanban-tracker-board'

export { WORKSPACE_KANBAN_TRACKER_LANES, getWorkspaceKanbanTrackerLane }

function sortBoardWorktrees(a: Worktree, b: Worktree): number {
  return b.lastActivityAt - a.lastActivityAt || a.displayName.localeCompare(b.displayName)
}

function sortManualBoardWorktrees(a: Worktree, b: Worktree): number {
  return (
    (b.manualOrder ?? b.sortOrder) - (a.manualOrder ?? a.sortOrder) ||
    a.displayName.localeCompare(b.displayName)
  )
}

export function groupWorkspaceKanbanWorktrees(params: {
  worktrees: readonly Worktree[]
  visibleWorktreeIds: ReadonlySet<string>
  /** @deprecated The Workspace board always uses its external tracker lanes. */
  workspaceStatuses?: readonly WorkspaceStatusDefinition[]
  confirmedPhases?: ReadonlyMap<string, TrackerPhaseWire>
  sortBy: SortBy
}): Map<WorkspaceStatus, Worktree[]> {
  const { worktrees, visibleWorktreeIds, confirmedPhases, sortBy } = params
  const grouped = new Map<WorkspaceStatus, Worktree[]>(
    WORKSPACE_KANBAN_TRACKER_LANES.map((status) => [status.id, []])
  )

  for (const worktree of worktrees) {
    if (!visibleWorktreeIds.has(worktree.id)) {
      continue
    }
    const lane = getWorkspaceKanbanTrackerLane(worktree, confirmedPhases)
    grouped.get(lane)!.push(worktree)
  }

  for (const items of grouped.values()) {
    items.sort(
      sortBy === 'manual'
        ? sortManualBoardWorktrees
        : (a, b) => Number(b.isPinned) - Number(a.isPinned) || sortBoardWorktrees(a, b)
    )
  }
  return grouped
}
