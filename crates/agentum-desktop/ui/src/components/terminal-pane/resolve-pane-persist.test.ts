import { describe, expect, it } from 'vitest'
import { resolvePaneUsesServerSession } from './resolve-pane-persist'

describe('resolvePaneUsesServerSession', () => {
  it('honors an explicit persist=true regardless of the global default', () => {
    expect(resolvePaneUsesServerSession({ tabPersistTmux: true, globalDefault: false })).toBe(true)
    expect(resolvePaneUsesServerSession({ tabPersistTmux: true, globalDefault: true })).toBe(true)
  })

  it('honors an explicit persist=false (ephemeral) regardless of the global default', () => {
    expect(resolvePaneUsesServerSession({ tabPersistTmux: false, globalDefault: true })).toBe(false)
    expect(resolvePaneUsesServerSession({ tabPersistTmux: false, globalDefault: false })).toBe(false)
  })

  it('falls back to the global default when the tab made no explicit choice', () => {
    expect(resolvePaneUsesServerSession({ tabPersistTmux: undefined, globalDefault: true })).toBe(
      true
    )
    expect(resolvePaneUsesServerSession({ tabPersistTmux: undefined, globalDefault: false })).toBe(
      false
    )
    expect(resolvePaneUsesServerSession({ tabPersistTmux: null, globalDefault: true })).toBe(true)
    expect(resolvePaneUsesServerSession({ tabPersistTmux: null, globalDefault: false })).toBe(false)
  })

  it('forces the server path for agent tabs even when persist=false', () => {
    // The local PTY stub injects the launch command into a shell, which for an
    // agent gets typed into the agent's own composer. Agents must use the
    // server path (which launches via the tool adapter) no matter the toggle.
    expect(
      resolvePaneUsesServerSession({
        tabPersistTmux: false,
        globalDefault: false,
        isAgentTab: true
      })
    ).toBe(true)
    expect(
      resolvePaneUsesServerSession({
        tabPersistTmux: undefined,
        globalDefault: false,
        isAgentTab: true
      })
    ).toBe(true)
  })

  it('leaves plain terminals on their explicit/default choice', () => {
    expect(
      resolvePaneUsesServerSession({
        tabPersistTmux: false,
        globalDefault: true,
        isAgentTab: false
      })
    ).toBe(false)
  })
})
