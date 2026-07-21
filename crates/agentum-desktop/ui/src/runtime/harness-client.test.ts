// Spec 023 Part B — the unlink client call (AC 7's "via runtime/harness-client.ts").
// `server-endpoint` is mocked so no loopback server is needed; `fetch` is
// stubbed per test.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { unlinkHarnessIssue } from './harness-client'

vi.mock('./server-endpoint', () => ({
  apiUrl: vi.fn((p: string) => Promise.resolve(p)),
  wsUrl: vi.fn((p: string) => Promise.resolve(p)),
  getServerEndpoint: vi.fn(() => Promise.resolve({ token: null }))
}))

afterEach(() => {
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe('unlinkHarnessIssue', () => {
  it('POSTs the dedicated unlink route for the run id', async () => {
    const fetchMock = vi.fn(() => Promise.resolve(new Response('', { status: 200 })))
    vi.stubGlobal('fetch', fetchMock)
    await unlinkHarnessIssue('run-1')
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(url).toBe('/api/harness/run-1/unlink-issue')
    expect(init.method).toBe('POST')
  })

  it('rejects with the server detail on a 404 (unknown run)', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(new Response('harness gone not found', { status: 404 })))
    )
    await expect(unlinkHarnessIssue('gone')).rejects.toThrow('harness 404')
  })
})
