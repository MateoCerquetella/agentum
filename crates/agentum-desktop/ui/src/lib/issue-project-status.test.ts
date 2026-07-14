import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  getCachedIssueProjectStatus,
  issueProjectStatusCacheKey,
  resetIssueProjectStatusCache
} from './issue-project-status'

// Spec 358b AC 3: fetched lazily once, cached per issue for the app session —
// a second hover (component remount) must trigger NO new fetch. AC 2: every
// failure resolves as null (silent absence), and stays cached as null.

describe('issueProjectStatusCacheKey', () => {
  it('keys by repoId when present, workdir otherwise', () => {
    expect(issueProjectStatusCacheKey({ workdir: '/w', repoId: 'r-1', number: 7 })).toBe('r-1::#7')
    expect(issueProjectStatusCacheKey({ workdir: '/w', number: 7 })).toBe('/w::#7')
  })

  it('separates issues within one repo', () => {
    expect(issueProjectStatusCacheKey({ workdir: '/w', repoId: 'r-1', number: 7 })).not.toBe(
      issueProjectStatusCacheKey({ workdir: '/w', repoId: 'r-1', number: 8 })
    )
  })
})

describe('getCachedIssueProjectStatus', () => {
  beforeEach(() => {
    resetIssueProjectStatusCache()
  })

  it('fetches once per issue and serves the session cache afterwards', async () => {
    const fetcher = vi.fn().mockResolvedValue({ status: 'In Progress' })
    const input = { workdir: '/w', repoId: 'r-1', number: 7 }

    await expect(getCachedIssueProjectStatus(input, fetcher)).resolves.toBe('In Progress')
    await expect(getCachedIssueProjectStatus(input, fetcher)).resolves.toBe('In Progress')
    expect(fetcher).toHaveBeenCalledTimes(1)
  })

  it('single-flights a concurrent second hover onto the pending fetch', async () => {
    let resolve!: (value: { status: string | null }) => void
    const fetcher = vi.fn().mockReturnValue(
      new Promise<{ status: string | null }>((r) => {
        resolve = r
      })
    )
    const input = { workdir: '/w', number: 7 }

    const first = getCachedIssueProjectStatus(input, fetcher)
    const second = getCachedIssueProjectStatus(input, fetcher)
    expect(fetcher).toHaveBeenCalledTimes(1)
    resolve({ status: 'Todo' })
    await expect(first).resolves.toBe('Todo')
    await expect(second).resolves.toBe('Todo')
  })

  it('fetches distinct issues independently', async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce({ status: 'Todo' })
      .mockResolvedValueOnce({ status: 'Done' })

    await expect(
      getCachedIssueProjectStatus({ workdir: '/w', number: 7 }, fetcher)
    ).resolves.toBe('Todo')
    await expect(
      getCachedIssueProjectStatus({ workdir: '/w', number: 8 }, fetcher)
    ).resolves.toBe('Done')
    expect(fetcher).toHaveBeenCalledTimes(2)
  })

  it('resolves a rejected fetch as null and caches the absence (no retry)', async () => {
    const fetcher = vi.fn().mockRejectedValue(new Error('gh: rate limited'))
    const input = { workdir: '/w', number: 7 }

    await expect(getCachedIssueProjectStatus(input, fetcher)).resolves.toBeNull()
    await expect(getCachedIssueProjectStatus(input, fetcher)).resolves.toBeNull()
    expect(fetcher).toHaveBeenCalledTimes(1)
  })

  it('passes a null status through unchanged (unbound repo / not on board)', async () => {
    const fetcher = vi.fn().mockResolvedValue({ status: null })
    await expect(
      getCachedIssueProjectStatus({ workdir: '/w', number: 7 }, fetcher)
    ).resolves.toBeNull()
  })
})
