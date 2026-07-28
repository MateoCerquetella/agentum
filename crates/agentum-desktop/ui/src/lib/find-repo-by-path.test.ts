import { describe, expect, it } from 'vitest'
import type { Repo } from '@/shared/types'
import { findRepoByPathPreferLocal } from './find-repo-by-path'

function makeRepo(overrides: Partial<Repo> & Pick<Repo, 'id' | 'path'>): Repo {
  return {
    displayName: overrides.path.split('/').pop() ?? overrides.path,
    badgeColor: '#5b8def',
    addedAt: 1,
    ...overrides
  }
}

describe('findRepoByPathPreferLocal', () => {
  it('returns undefined for an empty or missing registry', () => {
    expect(findRepoByPathPreferLocal(undefined, '/x/proj')).toBeUndefined()
    expect(findRepoByPathPreferLocal([], '/x/proj')).toBeUndefined()
  })

  it('returns undefined when no entry matches the path', () => {
    const repos = [makeRepo({ id: 'a', path: '/x/proj' })]
    expect(findRepoByPathPreferLocal(repos, '/y/other')).toBeUndefined()
  })

  it('returns a sole local match', () => {
    const local = makeRepo({ id: 'a', path: '/x/proj' })
    expect(findRepoByPathPreferLocal([local], '/x/proj')).toBe(local)
  })

  it('returns a sole remote match (remote-only registration)', () => {
    const remote = makeRepo({ id: 'a', path: '/x/proj', connectionId: 'ssh-1' })
    expect(findRepoByPathPreferLocal([remote], '/x/proj')).toBe(remote)
  })

  it('prefers the local entry over a remote dual entry, regardless of order', () => {
    const local = makeRepo({ id: 'local', path: '/x/proj' })
    const remote = makeRepo({ id: 'remote', path: '/x/proj', connectionId: 'ssh-1' })
    expect(findRepoByPathPreferLocal([remote, local], '/x/proj')).toBe(local)
    expect(findRepoByPathPreferLocal([local, remote], '/x/proj')).toBe(local)
  })

  it('treats an explicit null connectionId as local', () => {
    const local = makeRepo({ id: 'local', path: '/x/proj', connectionId: null })
    const remote = makeRepo({ id: 'remote', path: '/x/proj', connectionId: 'ssh-1' })
    expect(findRepoByPathPreferLocal([remote, local], '/x/proj')).toBe(local)
  })

  it('falls back to the first match when every entry is remote', () => {
    const first = makeRepo({ id: 'r1', path: '/x/proj', connectionId: 'ssh-1' })
    const second = makeRepo({ id: 'r2', path: '/x/proj', connectionId: 'ssh-2' })
    expect(findRepoByPathPreferLocal([first, second], '/x/proj')).toBe(first)
  })
})
