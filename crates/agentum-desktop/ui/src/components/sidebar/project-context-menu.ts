// Why: isolate the right-click open-decision and the macOS ctrl-click
// click-suppression window as pure functions so they are unit-testable in the
// node vitest environment (no DOM). Mirrors the shouldXxx predicates in
// WorktreeContextMenu.tsx.
import type { Repo } from '../../../../shared/types'
import type { WorktreeGroupBy } from './worktree-list-groups'

export type ProjectContextMenuTarget = { repo: Repo; x: number; y: number }

// macOS ctrl-click fires `contextmenu` AND a follow-up primary `click`. This
// window lets the row swallow that one trailing click so opening the menu does
// not also toggle/select the project. Same 500ms budget as WorktreeContextMenu.
const PROJECT_HEADER_CLICK_SUPPRESSION_MS = 500

// Why: project header rows carry a repo only under "repo" grouping and under
// "host" grouping (where projects render as repo sub-headers nested beneath
// host headers — the default view). Other groupings have no per-project header
// to anchor the menu to, so the right-click is a no-op there.
const PROJECT_HEADER_GROUPINGS: ReadonlySet<WorktreeGroupBy> = new Set(['repo', 'host'])

export function getProjectContextMenuTarget(args: {
  groupBy: WorktreeGroupBy
  repo: Repo | null | undefined
  clientX: number
  clientY: number
}): ProjectContextMenuTarget | null {
  if (!PROJECT_HEADER_GROUPINGS.has(args.groupBy) || args.repo == null) {
    return null
  }
  return { repo: args.repo, x: args.clientX, y: args.clientY }
}

export function shouldSuppressProjectHeaderClick(
  openedAt: number | null,
  now: number
): boolean {
  if (openedAt == null) {
    return false
  }
  const elapsed = now - openedAt
  return elapsed >= 0 && elapsed <= PROJECT_HEADER_CLICK_SUPPRESSION_MS
}
