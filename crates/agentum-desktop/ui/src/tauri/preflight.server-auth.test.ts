import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getJson } = vi.hoisted(() => ({ getJson: vi.fn() }))

vi.mock('@/runtime/server-http', () => ({ getJson }))

import { preflight } from './preflight'

describe('preflight embedded-server authentication', () => {
  beforeEach(() => {
    getJson.mockReset()
    getJson.mockResolvedValue({})
  })

  it('uses the shared bearer-authenticated HTTP client for every local probe', async () => {
    await preflight.check()
    await preflight.detectAgents()
    await preflight.refreshAgents()

    expect(getJson).toHaveBeenNthCalledWith(1, '/api/preflight/check')
    expect(getJson).toHaveBeenNthCalledWith(2, '/api/preflight/agents')
    expect(getJson).toHaveBeenNthCalledWith(3, '/api/preflight/agents/refresh')
  })
})
