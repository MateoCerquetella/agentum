// jsdom-free vitest for the spec-018 issue Project-status pure model (#365).
import { describe, expect, it, vi } from 'vitest'
import {
  parseIssueRef,
  resolveIssueProjectStatus,
  statusCacheKey,
  type IssueProjectStatusDeps,
  type IssueRef,
  type ProjectBindingRef
} from './issue-project-status'

describe('parseIssueRef', () => {
  it('parses a canonical issue URL into owner/repo/number/slug', () => {
    expect(parseIssueRef('https://github.com/MateoCerquetella/agentum/issues/365')).toEqual({
      owner: 'MateoCerquetella',
      repo: 'agentum',
      number: 365,
      slug: 'MateoCerquetella/agentum'
    })
  })

  it('tolerates a trailing slash, query, or fragment', () => {
    for (const suffix of ['', '/', '?foo=1', '#issuecomment-1']) {
      const ref = parseIssueRef(`https://github.com/o/r/issues/12${suffix}`)
      expect(ref).not.toBeNull()
      expect(ref?.number).toBe(12)
    }
  })

  it('returns null for non-issue / non-github / missing URLs', () => {
    for (const url of [
      undefined,
      null,
      '',
      'https://github.com/o/r/pull/5',
      'https://gitlab.com/o/r/issues/5',
      'https://github.com/o/r/issues/abc',
      'https://github.com/o/r/issues/0',
      'not a url'
    ]) {
      expect(parseIssueRef(url)).toBeNull()
    }
  })
})

describe('statusCacheKey', () => {
  it('keys by slug and number', () => {
    expect(statusCacheKey('o/r', 7)).toBe('o/r#7')
  })
})

const REF: IssueRef = { owner: 'o', repo: 'r', number: 7, slug: 'o/r' }
const BINDING: NonNullable<ProjectBindingRef> = { projectId: 'PVT_1', statusFieldId: 'F_status' }

function deps(
  over: Partial<IssueProjectStatusDeps> = {}
): IssueProjectStatusDeps & {
  getBinding: ReturnType<typeof vi.fn>
  getStatus: ReturnType<typeof vi.fn>
} {
  const getBinding = vi.fn(async () => BINDING as ProjectBindingRef)
  const getStatus = vi.fn(async () => 'In Progress' as string | null)
  return {
    bindingCache: new Map(),
    statusCache: new Map(),
    getBinding,
    getStatus,
    ...over
  } as IssueProjectStatusDeps & {
    getBinding: ReturnType<typeof vi.fn>
    getStatus: ReturnType<typeof vi.fn>
  }
}

describe('resolveIssueProjectStatus', () => {
  it('returns the Status option for a bound repo, filling both caches', async () => {
    const d = deps()
    expect(await resolveIssueProjectStatus(REF, d)).toBe('In Progress')
    expect(d.bindingCache.get('o/r')).toEqual(BINDING)
    expect(d.statusCache.get('o/r#7')?.status).toBe('In Progress')
  })

  it('does not refetch inside the freshness window (AC 3)', async () => {
    const d = deps()
    await resolveIssueProjectStatus(REF, d)
    await resolveIssueProjectStatus(REF, d)
    expect(d.getBinding).toHaveBeenCalledTimes(1)
    expect(d.getStatus).toHaveBeenCalledTimes(1)
  })

  it('revalidates a stale entry — a moved board column shows up (#379)', async () => {
    let clock = 0
    const getStatus = vi.fn(async () => 'Backlog' as string | null)
    const d = deps({ getStatus, now: () => clock, staleAfterMs: 1000 })
    expect(await resolveIssueProjectStatus(REF, d)).toBe('Backlog')
    getStatus.mockResolvedValue('In progress')
    clock = 999
    expect(await resolveIssueProjectStatus(REF, d)).toBe('Backlog') // still fresh
    clock = 1000
    expect(await resolveIssueProjectStatus(REF, d)).toBe('In progress') // stale → refetched
    expect(getStatus).toHaveBeenCalledTimes(2)
    expect(d.getBinding).toHaveBeenCalledTimes(1) // bound binding stays cached
  })

  it('keeps the last-known status when a revalidation fetch fails', async () => {
    let clock = 0
    const getStatus = vi.fn(async () => 'In Progress' as string | null)
    const d = deps({ getStatus, now: () => clock, staleAfterMs: 1000 })
    await resolveIssueProjectStatus(REF, d)
    getStatus.mockRejectedValue(new Error('gh flaked'))
    clock = 2000
    expect(await resolveIssueProjectStatus(REF, d)).toBe('In Progress')
  })

  it('re-probes an unbound repo on revalidation — binding later needs no restart', async () => {
    let clock = 0
    const getBinding = vi.fn(async () => null as ProjectBindingRef)
    const d = deps({ getBinding, now: () => clock, staleAfterMs: 1000 })
    expect(await resolveIssueProjectStatus(REF, d)).toBeNull()
    getBinding.mockResolvedValue(BINDING)
    clock = 2000
    expect(await resolveIssueProjectStatus(REF, d)).toBe('In Progress')
    expect(getBinding).toHaveBeenCalledTimes(2)
  })

  it('returns null and skips the status fetch when the repo is unbound (AC 2)', async () => {
    const d = deps({ getBinding: vi.fn(async () => null) })
    expect(await resolveIssueProjectStatus(REF, d)).toBeNull()
    expect(d.getStatus).not.toHaveBeenCalled()
    expect(d.statusCache.get('o/r#7')?.status).toBeNull()
  })

  it('returns null when the issue is not on the project (status null)', async () => {
    const d = deps({ getStatus: vi.fn(async () => null) })
    expect(await resolveIssueProjectStatus(REF, d)).toBeNull()
  })

  it('treats a binding fetch error as unbound — never throws (AC 2)', async () => {
    const d = deps({
      getBinding: vi.fn(async () => {
        throw new Error('network')
      })
    })
    await expect(resolveIssueProjectStatus(REF, d)).resolves.toBeNull()
  })

  it('treats a status fetch error as no-status — never throws (AC 2)', async () => {
    const d = deps({
      getStatus: vi.fn(async () => {
        throw new Error('gh failed')
      })
    })
    await expect(resolveIssueProjectStatus(REF, d)).resolves.toBeNull()
  })

  it('normalizes a blank option name to null', async () => {
    const d = deps({ getStatus: vi.fn(async () => '   ') })
    expect(await resolveIssueProjectStatus(REF, d)).toBeNull()
  })

  it('reuses a cached binding across different issues of the same repo', async () => {
    const d = deps()
    await resolveIssueProjectStatus(REF, d)
    await resolveIssueProjectStatus({ ...REF, number: 8 }, d)
    expect(d.getBinding).toHaveBeenCalledTimes(1)
    expect(d.getStatus).toHaveBeenCalledTimes(2)
  })
})
