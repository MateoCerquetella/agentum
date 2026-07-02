import { describe, expect, it } from 'vitest'

import { normalizeRepoSlug } from './chat-client'

describe('normalizeRepoSlug', () => {
  it('passes a bare owner/repo through (preserving case)', () => {
    expect(normalizeRepoSlug('owner/repo')).toBe('owner/repo')
    expect(normalizeRepoSlug('MateoCerquetella/Agentum')).toBe('MateoCerquetella/Agentum')
  })

  it('trims surrounding whitespace and trailing slash/.git', () => {
    expect(normalizeRepoSlug('  owner/repo  ')).toBe('owner/repo')
    expect(normalizeRepoSlug('owner/repo/')).toBe('owner/repo')
    expect(normalizeRepoSlug('owner/repo.git')).toBe('owner/repo')
  })

  it('extracts owner/repo from an https GitHub URL', () => {
    expect(normalizeRepoSlug('https://github.com/owner/repo')).toBe('owner/repo')
    expect(normalizeRepoSlug('https://github.com/owner/repo.git')).toBe('owner/repo')
    expect(normalizeRepoSlug('https://github.com/owner/repo/')).toBe('owner/repo')
  })

  it('extracts owner/repo from an ssh remote', () => {
    expect(normalizeRepoSlug('git@github.com:owner/repo.git')).toBe('owner/repo')
  })

  it('returns "" for blank/missing input so the caller omits repo_slug', () => {
    expect(normalizeRepoSlug('')).toBe('')
    expect(normalizeRepoSlug('   ')).toBe('')
    expect(normalizeRepoSlug(null)).toBe('')
    expect(normalizeRepoSlug(undefined)).toBe('')
  })
})
