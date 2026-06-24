import { describe, it, expect } from 'vitest'
import { paneRunsAgentForWordNav } from './word-nav-agent-detection'
import { AGENT_STATUS_STALE_AFTER_MS } from '../../../../shared/agent-status-types'

const NOW = 1_000_000_000

describe('paneRunsAgentForWordNav', () => {
  it('trusts a fresh explicit hook status regardless of title', () => {
    // A pane with live hook status is an agent even if its title is a plain dir.
    expect(paneRunsAgentForWordNav({ updatedAt: NOW }, '~/projects/app', NOW)).toBe(true)
  })

  it('ignores a stale hook status when the title is not an agent', () => {
    const stale = { updatedAt: NOW - AGENT_STATUS_STALE_AFTER_MS - 1 }
    expect(paneRunsAgentForWordNav(stale, 'mateo@mac: ~/dev', NOW)).toBe(false)
  })

  it('falls back to the OSC title when no hook status exists (Claude idle ✳)', () => {
    // Hooks are opt-in; an agent with no hook entry must still be detected so
    // option+←/→ sends the agent CSI, not readline \eb/\ef.
    expect(paneRunsAgentForWordNav(undefined, '✳ Building the thing', NOW)).toBe(true)
  })

  it('detects a Codex title with no hook status', () => {
    expect(paneRunsAgentForWordNav(undefined, 'Codex', NOW)).toBe(true)
  })

  it('detects a working braille-spinner agent title', () => {
    expect(paneRunsAgentForWordNav(undefined, '⠋ doing work', NOW)).toBe(true)
  })

  it('a stale hook status is rescued by a live agent title (idle agent editing)', () => {
    const stale = { updatedAt: NOW - AGENT_STATUS_STALE_AFTER_MS - 1 }
    expect(paneRunsAgentForWordNav(stale, '✳ awaiting input', NOW)).toBe(true)
  })

  it('returns false for a bare shell title and no hook status', () => {
    expect(paneRunsAgentForWordNav(undefined, 'mateo@mac: ~/projects/app', NOW)).toBe(false)
  })

  it('returns false when there is neither a hook status nor a title', () => {
    expect(paneRunsAgentForWordNav(undefined, undefined, NOW)).toBe(false)
  })
})
