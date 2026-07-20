import { describe, expect, it } from 'vitest'
import { bindingQuery } from './github-projects-client'

// Spec 020 F3: the binding GET/DELETE query builder. The pins matter at the
// wire — `repoId` present iff supplied (the server treats absent as local,
// pre-020 byte-for-byte), and the pre-020 workdir(+slug) shape is unchanged
// when no repoId is threaded.

describe('bindingQuery', () => {
  it('carries workdir alone when nothing else is supplied (pre-020 wire shape)', () => {
    const params = bindingQuery({ workdir: '/home/me/proj' })
    expect(params.get('workdir')).toBe('/home/me/proj')
    expect(params.has('slug')).toBe(false)
    expect(params.has('repoId')).toBe(false)
  })

  it('appends the slug hint when supplied', () => {
    const params = bindingQuery({ workdir: '/home/me/proj', slug: 'acme/widgets' })
    expect(params.get('workdir')).toBe('/home/me/proj')
    expect(params.get('slug')).toBe('acme/widgets')
    expect(params.has('repoId')).toBe(false)
  })

  it('appends repoId when supplied — the SSH-repo identity the host resolves from', () => {
    const params = bindingQuery({ workdir: '/srv/proj', repoId: 'repo-1' })
    expect(params.get('workdir')).toBe('/srv/proj')
    expect(params.get('repoId')).toBe('repo-1')
    expect(params.has('slug')).toBe(false)
  })

  it('carries all three together', () => {
    const params = bindingQuery({ workdir: '/srv/proj', slug: 'acme/widgets', repoId: 'repo-1' })
    expect(params.get('workdir')).toBe('/srv/proj')
    expect(params.get('slug')).toBe('acme/widgets')
    expect(params.get('repoId')).toBe('repo-1')
  })
})
