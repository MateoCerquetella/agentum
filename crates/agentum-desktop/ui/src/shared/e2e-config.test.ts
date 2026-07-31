import { describe, expect, it } from 'vitest'
import { createE2EConfig, loadE2EConfig } from './e2e-config'

const disabledConfig = {
  enabled: false,
  headless: false,
  exposeStore: false,
  userDataDir: null
}

describe('E2E config bridge normalization', () => {
  it('normalizes an absent native config instead of exposing null to startup modules', async () => {
    await expect(loadE2EConfig(() => Promise.resolve(null))).resolves.toEqual(disabledConfig)
    expect(createE2EConfig(undefined)).toEqual(disabledConfig)
  })

  it('uses the disabled config when the native bridge rejects', async () => {
    await expect(loadE2EConfig(() => Promise.reject(new Error('bridge unavailable')))).resolves.toEqual(
      disabledConfig
    )
  })

  it('normalizes a populated native config', async () => {
    await expect(
      loadE2EConfig(() => ({
        headless: true,
        exposeStore: true,
        userDataDir: '  /tmp/agentum-e2e  '
      }))
    ).resolves.toEqual({
      enabled: true,
      headless: true,
      exposeStore: true,
      userDataDir: '/tmp/agentum-e2e'
    })
  })
})
