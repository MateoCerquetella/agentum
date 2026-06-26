import {
  DEFAULT_HIDE_DEFAULT_BRANCH_WORKSPACE,
  DEFAULT_SHOW_SLEEPING_WORKSPACES
} from '../../../../shared/constants'

export type WorkspaceFilterSummary = {
  hasSleepingFilter: boolean
  hasDefaultBranchFilter: boolean
  hasRepoFilter: boolean
  hasAnyFilter: boolean
  activeFilterCount: number
}

/**
 * Derive whether any workspace filter deviates from its default, plus a count
 * for the badge. Shared by the sidebar header menu and the Kanban drawer filter
 * so the two surfaces can never drift apart — they did once: hiding the
 * default-branch row became the baseline, but the badge still counted it as an
 * active filter, leaving a phantom "1" with every toggle visually off.
 *
 * Each toggle is measured against its baseline (not against a hardcoded truthy
 * value), so the badge only lights up on an explicit, user-made deviation.
 */
export function deriveWorkspaceFilterSummary(input: {
  showSleepingWorkspaces: boolean
  hideDefaultBranchWorkspace: boolean
  selectedRepoCount: number
}): WorkspaceFilterSummary {
  const hasSleepingFilter = input.showSleepingWorkspaces !== DEFAULT_SHOW_SLEEPING_WORKSPACES
  const hasDefaultBranchFilter =
    input.hideDefaultBranchWorkspace !== DEFAULT_HIDE_DEFAULT_BRANCH_WORKSPACE
  const hasRepoFilter = input.selectedRepoCount > 0
  const hasAnyFilter = hasSleepingFilter || hasDefaultBranchFilter || hasRepoFilter
  const activeFilterCount =
    (hasSleepingFilter ? 1 : 0) + (hasDefaultBranchFilter ? 1 : 0) + input.selectedRepoCount
  return {
    hasSleepingFilter,
    hasDefaultBranchFilter,
    hasRepoFilter,
    hasAnyFilter,
    activeFilterCount
  }
}
