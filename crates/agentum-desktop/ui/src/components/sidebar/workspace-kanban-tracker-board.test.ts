import { describe, expect, it, vi } from 'vitest'
import type { Worktree } from '../../../../shared/types'
import {
  commitWorkspaceKanbanTrackerMove,
  getWorkspaceKanbanTrackerLane,
  refreshWorkspaceKanbanTrackerPhases
} from './workspace-kanban-tracker-board'

function worktree(overrides: Partial<Worktree> = {}): Worktree {
  return {
    id: 'repo::/workspace',
    repoId: 'repo',
    displayName: 'Workspace',
    comment: '',
    linkedIssue: 1,
    linkedPR: null,
    linkedLinearIssue: null,
    isArchived: false,
    isUnread: false,
    isPinned: false,
    sortOrder: 0,
    lastActivityAt: 0,
    path: '/workspace',
    branch: 'feature',
    head: 'abc',
    isBare: false,
    isMainWorktree: false,
    ...overrides
  }
}

describe('workspace tracker board', () => {
  it('places unsupported and incomplete bindings in Unlinked without consulting workspaceStatus', () => {
    expect(getWorkspaceKanbanTrackerLane(worktree({ workspaceStatus: 'done' }))).toBe('unlinked')
    expect(
      getWorkspaceKanbanTrackerLane(
        worktree({
          trackerProvider: 'github',
          trackerUrl: '',
          workspaceStatus: 'in_progress'
        })
      )
    ).toBe('unlinked')
  })

  it('prefers freshly reconciled provider evidence to cached trackerPhase', () => {
    const row = worktree({
      trackerProvider: 'linear',
      trackerUrl: 'ENG-42',
      trackerPhase: 'todo',
      workspaceStatus: 'done'
    })
    expect(getWorkspaceKanbanTrackerLane(row, new Map([[row.id, 'in_review']]))).toBe('in_review')
  })

  it('discards a stale refresh generation', async () => {
    const row = worktree({ trackerProvider: 'linear', trackerUrl: 'ENG-42' })
    const result = await refreshWorkspaceKanbanTrackerPhases({
      worktrees: [row],
      resolvePhase: async () => 'done',
      isCurrent: () => false
    })
    expect(result).toBeNull()
  })

  it('commits phase and rank only after an acknowledged external transition', async () => {
    const events: string[] = []
    await commitWorkspaceKanbanTrackerMove({
      worktreeId: 'repo::/workspace',
      targetPhase: 'done',
      transition: async () => {
        events.push('external')
        return { applied: true, phase: 'done' }
      },
      commitPhase: () => events.push('phase'),
      commitManualOrder: () => events.push('rank')
    })
    expect(events).toEqual(['external', 'phase', 'rank'])
  })

  it('preserves phase and rank when the tracker rejects the move', async () => {
    const commitPhase = vi.fn()
    const commitManualOrder = vi.fn()
    await expect(
      commitWorkspaceKanbanTrackerMove({
        worktreeId: 'repo::/workspace',
        targetPhase: 'done',
        transition: async () => {
          throw new Error('mapping unavailable')
        },
        commitPhase,
        commitManualOrder
      })
    ).rejects.toThrow('mapping unavailable')
    expect(commitPhase).not.toHaveBeenCalled()
    expect(commitManualOrder).not.toHaveBeenCalled()
  })
})
