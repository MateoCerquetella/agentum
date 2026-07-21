import { describe, expect, it } from 'vitest'
import { gatedRunResultOwnsWorktree } from './gated-run-ownership'

describe('gatedRunResultOwnsWorktree', () => {
  it('owns the worktree when a fresh run planned at least one feature', () => {
    expect(gatedRunResultOwnsWorktree({ planned: 3, alreadyRunning: false })).toBe(true)
    expect(gatedRunResultOwnsWorktree({ planned: 1, alreadyRunning: false })).toBe(true)
  })

  it('does NOT own the worktree when a fresh run planned zero features', () => {
    // The regression: planned:0 with runStarted:true used to suppress the plain
    // session, leaving the worktree stranded on an empty "Start a session".
    expect(gatedRunResultOwnsWorktree({ planned: 0, alreadyRunning: false })).toBe(false)
  })

  it('owns the worktree when a live run already drives it, regardless of plan count', () => {
    expect(gatedRunResultOwnsWorktree({ planned: 0, alreadyRunning: true })).toBe(true)
    expect(gatedRunResultOwnsWorktree({ planned: 5, alreadyRunning: true })).toBe(true)
  })
})
