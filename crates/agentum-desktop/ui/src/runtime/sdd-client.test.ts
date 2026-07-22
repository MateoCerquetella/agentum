import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('./server-http', () => ({
  getJson: vi.fn(),
  postJson: vi.fn()
}))

import { postJson } from './server-http'
import { injectSddPlaybook, setSddLoop } from './sdd-client'

beforeEach(() => vi.clearAllMocks())

describe('SDD session targeting', () => {
  it('addresses injection and loop actions only to the supplied pane session', async () => {
    vi.mocked(postJson)
      .mockResolvedValueOnce({ mode: 'bootstrap', ready: true })
      .mockResolvedValueOnce({ active: true, step: 1, max_steps: 10 })

    await injectSddPlaybook('codex-session', 'sdd-status')
    await setSddLoop('claude-session', true)

    expect(postJson).toHaveBeenNthCalledWith(
      1,
      '/api/sessions/codex-session/sdd/inject',
      { playbook: 'sdd-status' }
    )
    expect(postJson).toHaveBeenNthCalledWith(
      2,
      '/api/sessions/claude-session/sdd/loop',
      { active: true }
    )
  })
})
