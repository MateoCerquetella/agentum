import { describe, expect, it } from 'vitest'
import { deriveProjectRepoId } from './browser-project'

// Spec 014 AC 7: the workspace → project profile key derivation. Must mirror
// the server's BrowserScope chain for the two UI-decidable steps: a
// `<repoId>::…` id keys its repo, a bare repo UUID keys itself, everything
// else has NO project (so project-scoped actions are hidden, never mis-scoped).
describe('deriveProjectRepoId', () => {
  it('extracts the repo prefix from a full worktree id', () => {
    expect(deriveProjectRepoId('repo-abc::/Users/x/.agentum/worktrees/feat')).toBe('repo-abc')
  })

  it('handles folder-project workspace instance ids', () => {
    expect(
      deriveProjectRepoId('repo-abc::/folder::workspace:0123abcd-0000-0000-0000-000000000000')
    ).toBe('repo-abc')
  })

  it('accepts a bare repo uuid (path-less project surfaces)', () => {
    expect(deriveProjectRepoId('0123abcd-0000-0000-0000-000000000000')).toBe(
      '0123abcd-0000-0000-0000-000000000000'
    )
  })

  it('two worktrees of one repo derive the same project', () => {
    expect(deriveProjectRepoId('repo-a::/w/one')).toBe(deriveProjectRepoId('repo-a::/w/two'))
  })

  it('returns null for non-project contexts', () => {
    expect(deriveProjectRepoId('github-pr:repo:42')).toBeNull()
    expect(deriveProjectRepoId('global-floating-terminal')).toBeNull()
    expect(deriveProjectRepoId('__orphan__')).toBeNull()
    expect(deriveProjectRepoId('/bare/worktree/path')).toBeNull()
    expect(deriveProjectRepoId('::path-with-empty-prefix')).toBeNull()
    expect(deriveProjectRepoId('')).toBeNull()
    expect(deriveProjectRepoId('   ')).toBeNull()
    expect(deriveProjectRepoId(undefined)).toBeNull()
    expect(deriveProjectRepoId(null)).toBeNull()
  })
})
