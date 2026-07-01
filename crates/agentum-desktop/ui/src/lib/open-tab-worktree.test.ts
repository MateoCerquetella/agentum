import { describe, expect, it } from 'vitest'

import { resolveOpenTabWorktreeId } from './open-tab-worktree'

const candidates = [
  { id: 'repoA::/Users/me/proj', path: '/Users/me/proj' },
  {
    id: 'repoA::/Users/me/proj/.claude/worktrees/feat',
    path: '/Users/me/proj/.claude/worktrees/feat'
  },
  { id: 'repoB::/Users/me/www', path: '/Users/me/www' }
]

describe('resolveOpenTabWorktreeId', () => {
  it('matches the server-tagged bare PATH hint to its store worktree id', () => {
    // The server tags `?worktree=<workdir>` (a bare path); the store keys tabs
    // by `<repoId>::<path>`, so we resolve the path to the full id.
    expect(
      resolveOpenTabWorktreeId(
        '/Users/me/proj/.claude/worktrees/feat',
        candidates,
        'repoB::/Users/me/www'
      )
    ).toBe('repoA::/Users/me/proj/.claude/worktrees/feat')
  })

  it('uses an exact id match when a full id is passed', () => {
    expect(
      resolveOpenTabWorktreeId('repoB::/Users/me/www', candidates, 'repoA::/Users/me/proj')
    ).toBe('repoB::/Users/me/www')
  })

  it('matches by path portion when a full-id hint carries a different repoId', () => {
    expect(
      resolveOpenTabWorktreeId('other-repo::/Users/me/www', candidates, 'repoA::/Users/me/proj')
    ).toBe('repoB::/Users/me/www')
  })

  it('tolerates a trailing slash on either side of the path match', () => {
    expect(resolveOpenTabWorktreeId('/Users/me/www/', candidates, null)).toBe(
      'repoB::/Users/me/www'
    )
  })

  it('falls back to the active worktree when the hint is unresolvable', () => {
    expect(resolveOpenTabWorktreeId('/nowhere', candidates, 'repoA::/Users/me/proj')).toBe(
      'repoA::/Users/me/proj'
    )
  })

  it('falls back to the active worktree when there is no hint', () => {
    expect(resolveOpenTabWorktreeId(undefined, candidates, 'repoB::/Users/me/www')).toBe(
      'repoB::/Users/me/www'
    )
    expect(resolveOpenTabWorktreeId('   ', candidates, 'repoB::/Users/me/www')).toBe(
      'repoB::/Users/me/www'
    )
  })

  it('returns null only when there is neither a usable hint nor an active worktree', () => {
    expect(resolveOpenTabWorktreeId(null, candidates, null)).toBeNull()
  })
})
