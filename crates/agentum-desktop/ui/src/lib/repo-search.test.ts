import { describe, expect, it } from 'vitest'
import { searchRepos } from './repo-search'
import type { Repo } from '../../../shared/types'

function makeRepo(overrides: Partial<Repo> = {}): Repo {
  return {
    id: 'repo-1',
    path: '/Users/test/src/agentum',
    displayName: 'mateocerquetella/agentum',
    badgeColor: '#22c55e',
    addedAt: 0,
    ...overrides
  }
}

describe('repo-search', () => {
  it('returns all repos in original order for an empty query', () => {
    const repos = [
      makeRepo({ id: '1', displayName: 'alpha', path: '/tmp/alpha' }),
      makeRepo({ id: '2', displayName: 'beta', path: '/tmp/beta' })
    ]

    expect(searchRepos(repos, '')).toEqual(repos)
  })

  it('treats a whitespace-only query as an empty query', () => {
    const repos = [
      makeRepo({ id: '1', displayName: 'alpha', path: '/tmp/alpha' }),
      makeRepo({ id: '2', displayName: 'beta', path: '/tmp/beta' })
    ]

    expect(searchRepos(repos, '   ')).toEqual(repos)
  })

  it('matches display names case-insensitively', () => {
    const repos = [
      makeRepo({ id: '1', displayName: 'mateocerquetella/agentum', path: '/repos/agentum' }),
      makeRepo({ id: '2', displayName: 'mateocerquetella/noqa', path: '/repos/noqa' })
    ]

    expect(searchRepos(repos, 'AGENTUM').map((repo) => repo.id)).toEqual(['1'])
  })

  it('falls back to matching repo paths', () => {
    const repos = [
      makeRepo({ id: '1', displayName: 'frontend', path: '/src/team-a/agentum' }),
      makeRepo({ id: '2', displayName: 'backend', path: '/src/team-b/noqa' })
    ]

    expect(searchRepos(repos, 'team-a').map((repo) => repo.id)).toEqual(['1'])
  })

  it('keeps display-name matches ahead of path-only matches', () => {
    const repos = [
      makeRepo({ id: '1', displayName: 'misc', path: '/src/agentum-tools/misc' }),
      makeRepo({ id: '2', displayName: 'agentum', path: '/src/team-a/project' })
    ]

    expect(searchRepos(repos, 'agentum').map((repo) => repo.id)).toEqual(['2', '1'])
  })
})
