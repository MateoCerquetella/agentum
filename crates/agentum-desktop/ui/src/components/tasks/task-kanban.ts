// Provider-agnostic Kanban column model for the Tasks board.
//
// Tasks is the primary, external-backed Kanban (GitHub / Linear / GitLab). Each
// provider has its own status vocabulary, so we normalize to three canonical
// columns and provide the reverse mapping a drag-to-column needs to push the
// new state back to the tracker (two-way).

export type KanbanColumnKey = 'todo' | 'in_progress' | 'done'

export type KanbanColumn = { key: KanbanColumnKey; label: string }

/** The full 3-column model (Linear has all three; GitHub/GitLab use two). */
const KANBAN_COLUMNS: readonly KanbanColumn[] = [
  { key: 'todo', label: 'To Do' },
  { key: 'in_progress', label: 'In Progress' },
  { key: 'done', label: 'Done' }
]

/** GitHub/GitLab have no native "in progress", so they render two columns. */
export const TWO_COLUMNS: readonly KanbanColumn[] = [
  { key: 'todo', label: 'Open' },
  { key: 'done', label: 'Done' }
]

/** GitHub issue/PR `state` → column. open/draft are actionable; closed/merged done. */
export function githubColumn(state: string): KanbanColumnKey {
  return state === 'closed' || state === 'merged' ? 'done' : 'todo'
}

/** GitLab `state` → column. */
function gitlabColumn(state: string): KanbanColumnKey {
  return state === 'closed' || state === 'merged' || state === 'locked' ? 'done' : 'todo'
}

/**
 * Linear workflow-state *type* → column. Linear's `state.type` is a stable enum
 * (triage/backlog/unstarted/started/completed/canceled) even though `state.name`
 * is user-configurable — group on the type so columns are consistent across
 * teams. `started` is the only "in progress" signal.
 */
function linearColumn(stateType: string): KanbanColumnKey {
  switch (stateType) {
    case 'started':
      return 'in_progress'
    case 'completed':
    case 'canceled':
      return 'done'
    default:
      // triage | backlog | unstarted | unknown
      return 'todo'
  }
}

/**
 * The GitHub `state` to PATCH when a card is dragged to `target`. GitHub issues
 * only open/close, so `in_progress` is treated as "reopen" (open). Returns null
 * when the move is a no-op for this provider (avoid a pointless API call).
 */
export function githubTargetState(target: KanbanColumnKey): 'open' | 'closed' {
  return target === 'done' ? 'closed' : 'open'
}
