import type { WorkspaceStatusDefinition, Worktree } from '../../../../shared/types'
import { parseTrackerPhaseWire, type TrackerPhaseWire } from '../../lib/tracker-phase'

export const WORKSPACE_KANBAN_TRACKER_LANES = [
  { id: 'todo', label: 'Todo', color: 'neutral', icon: 'circle' },
  {
    id: 'in_progress',
    label: 'In Progress',
    color: 'conductor-progress',
    icon: 'conductor-progress'
  },
  {
    id: 'in_review',
    label: 'In Review',
    color: 'conductor-review',
    icon: 'conductor-review'
  },
  {
    id: 'ready_to_test',
    label: 'Ready to Test',
    color: 'amber',
    icon: 'circle-dot'
  },
  {
    id: 'done',
    label: 'Done',
    color: 'conductor-done',
    icon: 'conductor-done'
  },
  { id: 'unlinked', label: 'Unlinked', color: 'neutral', icon: 'circle-dashed' }
] as const satisfies readonly WorkspaceStatusDefinition[]

export type WorkspaceKanbanTrackerLane = TrackerPhaseWire | 'unlinked'

export function isWorkspaceKanbanLifecycleLane(lane: string): lane is TrackerPhaseWire {
  return parseTrackerPhaseWire(lane) !== null
}

export function hasWorkspaceKanbanTrackerLink(worktree: Worktree): boolean {
  return (
    (worktree.trackerProvider === 'github' || worktree.trackerProvider === 'linear') &&
    typeof worktree.trackerUrl === 'string' &&
    worktree.trackerUrl.trim().length > 0
  )
}

/** Resolve a board lane from confirmed provider evidence. Legacy
 * `workspaceStatus` is intentionally absent from this decision. */
export function getWorkspaceKanbanTrackerLane(
  worktree: Worktree,
  confirmedPhases?: ReadonlyMap<string, TrackerPhaseWire>
): WorkspaceKanbanTrackerLane {
  if (!hasWorkspaceKanbanTrackerLink(worktree)) {
    return 'unlinked'
  }
  return confirmedPhases?.get(worktree.id) ?? parseTrackerPhaseWire(worktree.trackerPhase) ?? 'todo'
}

/** Resolve all visible provider reads concurrently, then publish them only if
 * the drawer generation is still current. Individual failures retain the
 * card's prior confirmed phase instead of fabricating the requested state. */
export async function refreshWorkspaceKanbanTrackerPhases(input: {
  worktrees: readonly Worktree[]
  resolvePhase: (worktree: Worktree) => Promise<TrackerPhaseWire | null>
  isCurrent: () => boolean
}): Promise<Map<string, TrackerPhaseWire> | null> {
  const entries = await Promise.all(
    input.worktrees.map(async (worktree) => {
      try {
        return [worktree.id, await input.resolvePhase(worktree)] as const
      } catch {
        return [worktree.id, null] as const
      }
    })
  )
  if (!input.isCurrent()) {
    return null
  }
  return new Map(
    entries.filter((entry): entry is readonly [string, TrackerPhaseWire] => entry[1] !== null)
  )
}

/** Await the external write before exposing the returned canonical phase.
 * Rejections never invoke either local commit callback. */
export async function commitWorkspaceKanbanTrackerMove(input: {
  worktreeId: string
  targetPhase: TrackerPhaseWire
  transition: (
    worktreeId: string,
    targetPhase: TrackerPhaseWire
  ) => Promise<{
    applied: true
    phase: TrackerPhaseWire
  }>
  commitPhase: (phase: TrackerPhaseWire) => void
  commitManualOrder?: () => void | Promise<void>
}): Promise<TrackerPhaseWire> {
  const result = await input.transition(input.worktreeId, input.targetPhase)
  const phase = parseTrackerPhaseWire(result.phase)
  if (!result.applied || !phase) {
    throw new Error('Tracker transition was not acknowledged.')
  }
  input.commitPhase(phase)
  await input.commitManualOrder?.()
  return phase
}
