import { describe, expect, it, vi } from 'vitest'

import {
  canLaunchNewWork,
  initialNewWorkProgress,
  isNewWorkRetryAvailable,
  newWorkBusyLabel,
  newWorkPrimaryLabel,
  resolveLaunchIssue,
  updateNewWorkProgress
} from './new-work-launch-model'

describe('new workspace launch model', () => {
  it('tracks only issue and worktree creation; specs start later in Run Center', () => {
    expect(initialNewWorkProgress({}, 'new')).toEqual({ issue: 'pending', worktree: 'pending' })
    expect(initialNewWorkProgress({}, 'none')).toEqual({ issue: 'done', worktree: 'pending' })
  })

  it('does not advertise tracker-to-workspace launch copy', () => {
    expect(newWorkPrimaryLabel('none')).toBe('Create workspace')
    expect(newWorkPrimaryLabel('existing')).toBe('Create worktree')
    expect(newWorkPrimaryLabel('new')).toBe('Create issue')
  })

  it('reports durable workspace progress and retry state', () => {
    const active = updateNewWorkProgress(initialNewWorkProgress(), 'worktree', 'active')
    expect(newWorkBusyLabel(active)).toBe('Creating worktree…')
    expect(isNewWorkRetryAvailable(updateNewWorkProgress(active, 'worktree', 'error'), false)).toBe(true)
  })

  it('requires an agent and the selected issue source inputs', () => {
    expect(canLaunchNewWork({ source: 'none', hasSelectedAgent: true, canStageNewIssue: false, hasNewIssueTitle: false, hasSelectedIssue: false, hasIssueCheckpoint: false })).toBe(true)
    expect(canLaunchNewWork({ source: 'existing', hasSelectedAgent: true, canStageNewIssue: true, hasNewIssueTitle: false, hasSelectedIssue: false, hasIssueCheckpoint: false })).toBe(false)
    expect(canLaunchNewWork({ source: 'new', hasSelectedAgent: false, canStageNewIssue: true, hasNewIssueTitle: true, hasSelectedIssue: false, hasIssueCheckpoint: false })).toBe(false)
  })

  it('reuses a checkpoint instead of filing a duplicate issue', async () => {
    const item = { type: 'issue' as const, number: 1, title: 'One', url: 'https://example.test/1' }
    const createIssue = vi.fn()
    const result = await resolveLaunchIssue({ source: 'new', checkpoint: { linkedWorkItem: item }, createIssue })
    expect(result.issue).toBe(item)
    expect(createIssue).not.toHaveBeenCalled()
  })
})
