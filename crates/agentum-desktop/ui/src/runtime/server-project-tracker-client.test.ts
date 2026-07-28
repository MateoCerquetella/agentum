import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ProjectTrackerConfig } from '@/shared/project-tracker-config'
import {
  getProjectTrackerConfig,
  ProjectTrackerConflictError,
  putProjectTrackerConfig
} from './server-project-tracker-client'

vi.mock('./server-endpoint', () => ({
  apiUrl: vi.fn(async (path: string) => `http://agentum.test${path}`),
  getServerEndpoint: vi.fn(async () => ({ url: 'http://agentum.test', token: 'secret' }))
}))

function config(repoId = 'repo-a', revision = 3): ProjectTrackerConfig {
  return {
    schemaVersion: 1,
    repoId,
    revision,
    provider: 'github',
    github: { repositorySlug: 'acme/widgets' },
    taskPreferences: {},
    provenance: 'configured'
  }
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' }
  })
}

describe('server project tracker client', () => {
  beforeEach(() => vi.stubGlobal('fetch', vi.fn()))
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.clearAllMocks()
  })

  it('reads the canonical repo-keyed config with authentication', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(
      jsonResponse({ config: config(), migrationConflict: 'review legacy pin' })
    )

    await expect(getProjectTrackerConfig('repo-a')).resolves.toEqual({
      config: config(),
      migrationConflict: 'review legacy pin'
    })
    expect(fetch).toHaveBeenCalledWith(
      'http://agentum.test/api/repos/repo-a/tracker-config',
      expect.objectContaining({ headers: expect.objectContaining({ Authorization: 'Bearer secret' }) })
    )
  })

  it('writes with the cached revision as a compare-and-swap precondition', async () => {
    const saved = config('repo-a', 4)
    vi.mocked(fetch).mockResolvedValueOnce(jsonResponse(saved))

    await expect(putProjectTrackerConfig('repo-a', config(), 3)).resolves.toEqual(saved)
    const init = vi.mocked(fetch).mock.calls[0][1] as RequestInit
    expect(init.method).toBe('PUT')
    expect(JSON.parse(String(init.body))).toEqual({ expectedRevision: 3, config: config() })
  })

  it('returns the authoritative current record on a CAS conflict', async () => {
    const current = config('repo-a', 9)
    vi.mocked(fetch).mockResolvedValueOnce(
      jsonResponse({ error: 'tracker config revision conflict', current }, 409)
    )

    const error = await putProjectTrackerConfig('repo-a', config(), 3).catch((cause) => cause)
    expect(error).toBeInstanceOf(ProjectTrackerConflictError)
    expect((error as ProjectTrackerConflictError).current).toEqual(current)
  })

  it('rejects a response that belongs to another repo', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonResponse({ config: config('repo-b') }))
    await expect(getProjectTrackerConfig('repo-a')).rejects.toThrow(
      'does not belong to the requested project'
    )
  })
})
