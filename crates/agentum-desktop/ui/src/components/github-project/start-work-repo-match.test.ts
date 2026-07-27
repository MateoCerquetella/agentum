import { describe, expect, it } from 'vitest'
import type { Repo } from '@/shared/types'
import { classifyStartWorkRepoMatches } from './start-work-repo-match'

function makeRepo(overrides: Partial<Repo> & Pick<Repo, 'id'>): Repo {
  return {
    path: '/x/proj',
    displayName: 'proj',
    badgeColor: '#5b8def',
    addedAt: 1,
    ...overrides
  }
}

describe('classifyStartWorkRepoMatches', () => {
  it('classifies an empty match set as none (missing-repo dialog)', () => {
    expect(classifyStartWorkRepoMatches([])).toEqual({ kind: 'none' })
  })

  it('classifies a sole local match as direct', () => {
    const local = makeRepo({ id: 'local' })
    const result = classifyStartWorkRepoMatches([local])
    expect(result).toEqual({ kind: 'direct', repo: local })
  })

  it('classifies a sole remote match as direct (VPS-only repo starts on the VPS)', () => {
    const remote = makeRepo({ id: 'remote', connectionId: 'ssh-1' })
    const result = classifyStartWorkRepoMatches([remote])
    expect(result).toEqual({ kind: 'direct', repo: remote })
  })

  it('classifies local + remote as choose seeded with the local copy, regardless of order', () => {
    const local = makeRepo({ id: 'local' })
    const remote = makeRepo({ id: 'remote', connectionId: 'ssh-1' })

    expect(classifyStartWorkRepoMatches([remote, local])).toEqual({
      kind: 'choose',
      repos: [remote, local],
      seedRepoId: 'local'
    })
    expect(classifyStartWorkRepoMatches([local, remote])).toEqual({
      kind: 'choose',
      repos: [local, remote],
      seedRepoId: 'local'
    })
  })

  it('treats an explicit null connectionId as local for seeding', () => {
    const local = makeRepo({ id: 'local', connectionId: null })
    const remote = makeRepo({ id: 'remote', connectionId: 'ssh-1' })
    const result = classifyStartWorkRepoMatches([remote, local])
    expect(result.kind).toBe('choose')
    expect(result.kind === 'choose' && result.seedRepoId).toBe('local')
  })

  it('seeds with the first match when every candidate is remote', () => {
    const first = makeRepo({ id: 'r1', connectionId: 'ssh-1' })
    const second = makeRepo({ id: 'r2', connectionId: 'ssh-2' })
    expect(classifyStartWorkRepoMatches([first, second])).toEqual({
      kind: 'choose',
      repos: [first, second],
      seedRepoId: 'r1'
    })
  })

  it('is deterministic and does not mutate its input', () => {
    const local = makeRepo({ id: 'local' })
    const remote = makeRepo({ id: 'remote', connectionId: 'ssh-1' })
    const matches = [remote, local]
    const snapshot = [...matches]

    const first = classifyStartWorkRepoMatches(matches)
    const second = classifyStartWorkRepoMatches(matches)

    expect(first).toEqual(second)
    expect(matches).toEqual(snapshot)
  })
})
