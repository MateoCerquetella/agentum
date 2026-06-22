import { describe, expect, it } from 'vitest'

import { branchDisplayName } from './WorktreeCardHelpers'

describe('branchDisplayName', () => {
  it('strips the refs/heads/ prefix and passes plain names through', () => {
    expect(branchDisplayName('refs/heads/feat/foo')).toBe('feat/foo')
    expect(branchDisplayName('main')).toBe('main')
  })

  // Regression: Worktree.branch is *typed* `string`, but at runtime it is
  // null for an SSH worktree whose branch is unresolved (e.g. the host is
  // disconnected). Calling `.replace` on null threw
  // "null is not an object (evaluating 'e.replace')" and crashed the whole
  // worktree-list sidebar (boundary sidebar.worktrees). It must degrade to ''.
  it('does not throw on a null/undefined branch (SSH-disconnected worktree)', () => {
    expect(branchDisplayName(null as unknown as string)).toBe('')
    expect(branchDisplayName(undefined as unknown as string)).toBe('')
  })
})
