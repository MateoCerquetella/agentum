import { describe, expect, it } from 'vitest'
import { resolveBoardRoute } from './board-route'

const REPOS = [
  { id: 'git-a', kind: 'git' as const },
  { id: 'git-b', kind: undefined }, // kind absent defaults to git
  { id: 'folder-c', kind: 'folder' as const }
]

describe('resolveBoardRoute', () => {
  it('the preferred repo wins over the active repo', () => {
    expect(
      resolveBoardRoute({ repos: REPOS, preferredRepoId: 'git-a', activeRepoId: 'git-b' })
    ).toEqual({ kind: 'hub', repoId: 'git-a' })
  })

  it('a stale preferred id falls to the active repo', () => {
    expect(
      resolveBoardRoute({ repos: REPOS, preferredRepoId: 'removed', activeRepoId: 'git-b' })
    ).toEqual({ kind: 'hub', repoId: 'git-b' })
  })

  it('a stale active id falls to the Projects page (no first-git-repo fallback)', () => {
    expect(resolveBoardRoute({ repos: REPOS, activeRepoId: 'removed' })).toEqual({
      kind: 'projects'
    })
  })

  it('non-git repos are excluded from both tiers', () => {
    expect(
      resolveBoardRoute({ repos: REPOS, preferredRepoId: 'folder-c', activeRepoId: 'folder-c' })
    ).toEqual({ kind: 'projects' })
  })

  it('no repos at all routes to the Projects page', () => {
    expect(
      resolveBoardRoute({ repos: [], preferredRepoId: 'git-a', activeRepoId: 'git-a' })
    ).toEqual({ kind: 'projects' })
  })

  it('null ids (cold start) route to the Projects page', () => {
    expect(resolveBoardRoute({ repos: REPOS, activeRepoId: null })).toEqual({ kind: 'projects' })
  })
})
