import { describe, expect, it } from 'vitest'
import { viewAfterWorktreeClose } from './worktree-close-view'

describe('viewAfterWorktreeClose', () => {
  it('redirects to Mission Control (activity) when the active worktree closes from the workspace view', () => {
    // The bug: after close, terminal + no active worktree renders the squeezed
    // Mission Control fallback. Land on 'activity' (right-sidebar-suppressed).
    expect(viewAfterWorktreeClose(true, 'terminal')).toBe('activity')
  })

  it('leaves the view unchanged when a non-active (background) worktree closes', () => {
    expect(viewAfterWorktreeClose(false, 'terminal')).toBe('terminal')
  })

  it('is a no-op when already on Mission Control', () => {
    expect(viewAfterWorktreeClose(true, 'activity')).toBe('activity')
  })

  it('never yanks the user off a non-terminal view when the active worktree closes', () => {
    expect(viewAfterWorktreeClose(true, 'settings')).toBe('settings')
    expect(viewAfterWorktreeClose(true, 'tasks')).toBe('tasks')
    expect(viewAfterWorktreeClose(true, 'projects')).toBe('projects')
    expect(viewAfterWorktreeClose(true, 'project')).toBe('project')
    expect(viewAfterWorktreeClose(true, 'harness')).toBe('harness')
  })
})
