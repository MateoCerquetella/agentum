import { describe, expect, it } from 'vitest'
import { slugResolutionArm } from './repo-slug-arm'

describe('slugResolutionArm', () => {
  it('keeps the environment RPC arm even for SSH repos (spec 020 non-goal: RPC untouched)', () => {
    expect(slugResolutionArm(true, 'ssh-1')).toBe('environment-rpc')
  })

  it('keeps the environment RPC arm for local repos', () => {
    expect(slugResolutionArm(true, null)).toBe('environment-rpc')
    expect(slugResolutionArm(true, undefined)).toBe('environment-rpc')
  })

  it('routes connectionId-bearing repos to the server host-aware arm (spec 020 F2)', () => {
    expect(slugResolutionArm(false, 'ssh-1')).toBe('server')
  })

  it('keeps local repos on the native arm — null and undefined connectionId alike', () => {
    expect(slugResolutionArm(false, null)).toBe('native')
    expect(slugResolutionArm(false, undefined)).toBe('native')
  })
})
