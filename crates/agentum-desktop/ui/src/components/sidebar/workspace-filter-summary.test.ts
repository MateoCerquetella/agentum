import { describe, expect, it } from 'vitest'
import {
  DEFAULT_HIDE_DEFAULT_BRANCH_WORKSPACE,
  DEFAULT_SHOW_SLEEPING_WORKSPACES
} from '@/shared/constants'
import { deriveWorkspaceFilterSummary } from './workspace-filter-summary'

describe('deriveWorkspaceFilterSummary', () => {
  it('reports no active filter in the default state (regression: phantom "1" badge)', () => {
    // The defaults hide the default-branch workspace; that baseline must NOT
    // count as an active filter, or the badge shows "1" with every toggle off.
    const summary = deriveWorkspaceFilterSummary({
      showSleepingWorkspaces: DEFAULT_SHOW_SLEEPING_WORKSPACES,
      hideDefaultBranchWorkspace: DEFAULT_HIDE_DEFAULT_BRANCH_WORKSPACE,
      selectedRepoCount: 0
    })
    expect(summary.hasAnyFilter).toBe(false)
    expect(summary.activeFilterCount).toBe(0)
    expect(summary.hasDefaultBranchFilter).toBe(false)
  })

  it('counts showing default branches as a deviation from the baseline', () => {
    const summary = deriveWorkspaceFilterSummary({
      showSleepingWorkspaces: DEFAULT_SHOW_SLEEPING_WORKSPACES,
      hideDefaultBranchWorkspace: !DEFAULT_HIDE_DEFAULT_BRANCH_WORKSPACE,
      selectedRepoCount: 0
    })
    expect(summary.hasDefaultBranchFilter).toBe(true)
    expect(summary.hasAnyFilter).toBe(true)
    expect(summary.activeFilterCount).toBe(1)
  })

  it('counts hiding sleeping workspaces as an active filter', () => {
    const summary = deriveWorkspaceFilterSummary({
      showSleepingWorkspaces: !DEFAULT_SHOW_SLEEPING_WORKSPACES,
      hideDefaultBranchWorkspace: DEFAULT_HIDE_DEFAULT_BRANCH_WORKSPACE,
      selectedRepoCount: 0
    })
    expect(summary.hasSleepingFilter).toBe(true)
    expect(summary.activeFilterCount).toBe(1)
  })

  it('adds the selected repo count and combines with toggle deviations', () => {
    const summary = deriveWorkspaceFilterSummary({
      showSleepingWorkspaces: !DEFAULT_SHOW_SLEEPING_WORKSPACES,
      hideDefaultBranchWorkspace: !DEFAULT_HIDE_DEFAULT_BRANCH_WORKSPACE,
      selectedRepoCount: 3
    })
    expect(summary.hasRepoFilter).toBe(true)
    expect(summary.hasAnyFilter).toBe(true)
    // sleeping (1) + default-branch (1) + repos (3)
    expect(summary.activeFilterCount).toBe(5)
  })

  it('treats a selected repo count alone as an active filter', () => {
    const summary = deriveWorkspaceFilterSummary({
      showSleepingWorkspaces: DEFAULT_SHOW_SLEEPING_WORKSPACES,
      hideDefaultBranchWorkspace: DEFAULT_HIDE_DEFAULT_BRANCH_WORKSPACE,
      selectedRepoCount: 2
    })
    expect(summary.hasRepoFilter).toBe(true)
    expect(summary.hasAnyFilter).toBe(true)
    expect(summary.activeFilterCount).toBe(2)
  })
})
