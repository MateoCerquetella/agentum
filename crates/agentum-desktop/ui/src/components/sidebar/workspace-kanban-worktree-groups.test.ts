import { describe, expect, it } from 'vitest'
import type { Worktree } from '../../../../shared/types'
import {
  WORKSPACE_KANBAN_TRACKER_LANES,
  groupWorkspaceKanbanWorktrees
} from './workspace-kanban-worktree-groups'

function worktree({
  id,
  displayName,
  ...overrides
}: Partial<Worktree> & Pick<Worktree, 'id' | 'displayName'>): Worktree {
  return {
    repoId: 'repo',
    path: `/tmp/${id}`,
    head: 'head',
    branch: displayName,
    isBare: false,
    isMainWorktree: false,
    id,
    displayName,
    comment: '',
    linkedIssue: null,
    linkedPR: null,
    linkedLinearIssue: null,
    isArchived: false,
    isUnread: false,
    isPinned: false,
    sortOrder: 0,
    lastActivityAt: 0,
    ...overrides
  } as Worktree
}

function linked(overrides: Partial<Worktree> & Pick<Worktree, 'id' | 'displayName'>): Worktree {
  return worktree({
    trackerProvider: 'github',
    trackerUrl: `https://github.com/acme/repo/issues/${overrides.id}`,
    ...overrides
  })
}

describe('groupWorkspaceKanbanWorktrees', () => {
  it('renders the five canonical tracker lanes in order', () => {
    const grouped = groupWorkspaceKanbanWorktrees({
      worktrees: [],
      visibleWorktreeIds: new Set(),
      workspaceStatuses: [{ id: 'private-local-state', label: 'Private local state' }],
      sortBy: 'recent'
    })

    expect(WORKSPACE_KANBAN_TRACKER_LANES.map(({ id, label }) => ({ id, label }))).toEqual([
      { id: 'todo', label: 'Todo' },
      { id: 'in_progress', label: 'In Progress' },
      { id: 'in_review', label: 'In Review' },
      { id: 'ready_to_test', label: 'Ready to Test' },
      { id: 'done', label: 'Done' },
      { id: 'unlinked', label: 'Unlinked' }
    ])
    expect(Array.from(grouped.keys())).toEqual([
      'todo',
      'in_progress',
      'in_review',
      'ready_to_test',
      'done',
      'unlinked'
    ])
  })

  it('renders incomplete bindings in the explicit Unlinked lane', () => {
    const grouped = groupWorkspaceKanbanWorktrees({
      worktrees: [
        worktree({
          id: 'local',
          displayName: 'Local',
          workspaceStatus: 'done'
        })
      ],
      visibleWorktreeIds: new Set(['local']),
      sortBy: 'recent'
    })

    expect(grouped.get('unlinked')?.map((item) => item.id)).toEqual(['local'])
    expect(grouped.get('done')).toEqual([])
  })

  it('uses the confirmed tracker phase instead of a contradictory workspace status', () => {
    const grouped = groupWorkspaceKanbanWorktrees({
      worktrees: [
        linked({
          id: 'todo',
          displayName: 'Todo',
          trackerPhase: 'todo',
          workspaceStatus: 'done'
        }),
        linked({
          id: 'progress',
          displayName: 'Progress',
          trackerProvider: 'linear',
          trackerUrl: 'ENG-41',
          trackerPhase: 'in_progress',
          workspaceStatus: 'todo'
        }),
        linked({
          id: 'github',
          displayName: 'GitHub',
          trackerPhase: 'in_review',
          workspaceStatus: 'done'
        }),
        linked({
          id: 'linear',
          displayName: 'Linear',
          trackerProvider: 'linear',
          trackerUrl: 'ENG-42',
          trackerPhase: 'ready_to_test',
          workspaceStatus: 'todo'
        }),
        linked({
          id: 'done',
          displayName: 'Done',
          trackerPhase: 'done',
          workspaceStatus: 'in-progress'
        })
      ],
      visibleWorktreeIds: new Set(['todo', 'progress', 'github', 'linear', 'done']),
      sortBy: 'recent'
    })

    expect(grouped.get('todo')?.map((item) => item.id)).toEqual(['todo'])
    expect(grouped.get('in_progress')?.map((item) => item.id)).toEqual(['progress'])
    expect(grouped.get('in_review')?.map((item) => item.id)).toEqual(['github'])
    expect(grouped.get('ready_to_test')?.map((item) => item.id)).toEqual(['linear'])
    expect(grouped.get('done')?.map((item) => item.id)).toEqual(['done'])
  })

  it('uses Todo as the baseline for a linked workspace without a confirmed phase', () => {
    const grouped = groupWorkspaceKanbanWorktrees({
      worktrees: [
        linked({
          id: 'new',
          displayName: 'New',
          trackerPhase: null,
          workspaceStatus: 'done'
        })
      ],
      visibleWorktreeIds: new Set(['new']),
      sortBy: 'recent'
    })

    expect(grouped.get('todo')?.map((item) => item.id)).toEqual(['new'])
  })

  it('uses manualOrder inside lanes when Manual sort is active', () => {
    const grouped = groupWorkspaceKanbanWorktrees({
      worktrees: [
        linked({
          id: 'a',
          displayName: 'A',
          trackerPhase: 'in_progress',
          manualOrder: 100,
          lastActivityAt: 10
        }),
        linked({
          id: 'b',
          displayName: 'B',
          trackerPhase: 'in_progress',
          manualOrder: 300,
          lastActivityAt: 1
        }),
        linked({
          id: 'c',
          displayName: 'C',
          trackerPhase: 'in_progress',
          manualOrder: 200,
          lastActivityAt: 50
        })
      ],
      visibleWorktreeIds: new Set(['a', 'b', 'c']),
      sortBy: 'manual'
    })

    expect(grouped.get('in_progress')?.map((item) => item.id)).toEqual(['b', 'c', 'a'])
  })

  it('keeps pinned then recent ordering outside Manual sort', () => {
    const grouped = groupWorkspaceKanbanWorktrees({
      worktrees: [
        linked({
          id: 'a',
          displayName: 'A',
          trackerPhase: 'in_progress',
          isPinned: false,
          lastActivityAt: 50
        }),
        linked({
          id: 'b',
          displayName: 'B',
          trackerPhase: 'in_progress',
          isPinned: true,
          lastActivityAt: 1
        })
      ],
      visibleWorktreeIds: new Set(['a', 'b']),
      sortBy: 'recent'
    })

    expect(grouped.get('in_progress')?.map((item) => item.id)).toEqual(['b', 'a'])
  })
})
