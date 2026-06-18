import { describe, expect, it } from 'vitest'
import {
  deriveEligibleHosts,
  filterReposForHost,
  gitOnHostCacheKey,
  resolveDefaultHostKey,
  resolveRepoIdForHost
} from './composer-host-scoping'
import type { Repo } from '../../../shared/types'
import type { HostKey, HostMeta } from '@/store/slices/hosts'

function repo(id: string, connectionId?: string): Repo {
  return {
    id,
    path: `/p/${id}`,
    displayName: id,
    badgeColor: '#000',
    addedAt: 0,
    ...(connectionId ? { connectionId } : {})
  }
}

function meta(key: HostKey, kind: 'local' | 'ssh', label: string): HostMeta {
  return { key, kind, label }
}

describe('deriveEligibleHosts', () => {
  it('returns distinct hosts, local first, labelled from hostMetaByKey', () => {
    const repos = [repo('a'), repo('b', 'conn-1'), repo('c', 'conn-1'), repo('d', 'conn-2')]
    const hostMetaByKey: Record<HostKey, HostMeta> = {
      local: meta('local', 'local', 'studio'),
      'ssh:conn-1': meta('ssh:conn-1', 'ssh', 'forge'),
      'ssh:conn-2': meta('ssh:conn-2', 'ssh', 'vps')
    }
    expect(deriveEligibleHosts(repos, hostMetaByKey)).toEqual([
      { key: 'local', kind: 'local', label: 'studio' },
      { key: 'ssh:conn-1', kind: 'ssh', label: 'forge' },
      { key: 'ssh:conn-2', kind: 'ssh', label: 'vps' }
    ])
  })

  it('keeps local first even when an ssh repo appears before any local repo', () => {
    const repos = [repo('b', 'conn-1'), repo('a')]
    const hosts = deriveEligibleHosts(repos, {})
    expect(hosts.map((h) => h.key)).toEqual(['local', 'ssh:conn-1'])
  })

  it('falls back to kind-derived labels when meta has not hydrated yet', () => {
    const repos = [repo('a'), repo('b', 'conn-1')]
    expect(deriveEligibleHosts(repos, {})).toEqual([
      { key: 'local', kind: 'local', label: 'This machine' },
      { key: 'ssh:conn-1', kind: 'ssh', label: 'SSH host' }
    ])
  })

  it('returns an empty list when there are no eligible repos', () => {
    expect(deriveEligibleHosts([], {})).toEqual([])
  })
})

describe('filterReposForHost', () => {
  const repos = [repo('a'), repo('b'), repo('c', 'conn-1'), repo('d', 'conn-2')]

  it('returns local repos (no connectionId) for the local host key', () => {
    expect(filterReposForHost(repos, 'local').map((r) => r.id)).toEqual(['a', 'b'])
  })

  it('returns only the matching ssh host repos', () => {
    expect(filterReposForHost(repos, 'ssh:conn-1').map((r) => r.id)).toEqual(['c'])
  })
})

describe('resolveDefaultHostKey', () => {
  const repos = [repo('a'), repo('c', 'conn-1')]
  const hosts = deriveEligibleHosts(repos, {})

  it("defaults to the active repo's host when it is eligible", () => {
    expect(resolveDefaultHostKey(repos, 'c', hosts)).toBe('ssh:conn-1')
  })

  it('defaults to the first eligible host when there is no active repo', () => {
    expect(resolveDefaultHostKey(repos, null, hosts)).toBe('local')
  })

  it('defaults to the first eligible host when the active repo is not eligible', () => {
    expect(resolveDefaultHostKey(repos, 'missing', hosts)).toBe('local')
  })

  it('falls back to local when there are no eligible hosts', () => {
    expect(resolveDefaultHostKey([], 'x', [])).toBe('local')
  })
})

describe('resolveRepoIdForHost', () => {
  const scoped = [repo('a'), repo('b')]

  it('keeps the current repoId when it belongs to the scoped repos', () => {
    expect(resolveRepoIdForHost(scoped, 'b')).toBe('b')
  })

  it("resets to the host's first repo when the current repoId is from another host", () => {
    expect(resolveRepoIdForHost(scoped, 'c')).toBe('a')
  })

  it('resets to empty string when the host has no repos', () => {
    expect(resolveRepoIdForHost([], 'a')).toBe('')
  })

  it('selects the first repo when no current selection exists', () => {
    expect(resolveRepoIdForHost(scoped, '')).toBe('a')
  })
})

describe('gitOnHostCacheKey', () => {
  it('namespaces a repoId by its host so the same repo can differ per host', () => {
    expect(gitOnHostCacheKey('local', 'repo-1')).toBe('local::repo-1')
    expect(gitOnHostCacheKey('ssh:conn-1', 'repo-1')).toBe('ssh:conn-1::repo-1')
    expect(gitOnHostCacheKey('local', 'repo-1')).not.toBe(gitOnHostCacheKey('ssh:conn-1', 'repo-1'))
  })
})
