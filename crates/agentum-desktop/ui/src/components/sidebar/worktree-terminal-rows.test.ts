import { describe, expect, it } from 'vitest'
import type { TerminalTab } from '@/shared/types'
import { buildWorktreeTerminalRows } from './worktree-terminal-rows'

function makeTab(overrides: Partial<TerminalTab> & { id: string }): TerminalTab {
  return {
    worktreeId: 'wt-1',
    ptyId: null,
    title: 'Terminal 1',
    customTitle: null,
    color: null,
    sortOrder: 0,
    createdAt: 0,
    ...overrides
  }
}

describe('buildWorktreeTerminalRows', () => {
  it('lists a freshly-created plain terminal even without a live PTY', () => {
    // The core bug: a new terminal has ptyId null and no ptyIdsByTabId entry
    // yet, but it must still appear in the sidebar.
    const rows = buildWorktreeTerminalRows({
      tabs: [makeTab({ id: 'tab-new', title: 'Terminal 2' })],
      agentTabIds: new Set(),
      ptyIdsByTabId: {}
    })

    expect(rows.map((r) => r.tabId)).toEqual(['tab-new'])
    expect(rows[0]?.title).toBe('Terminal 2')
  })

  it('excludes tabs already represented by an agent row', () => {
    const rows = buildWorktreeTerminalRows({
      tabs: [
        makeTab({ id: 'tab-agent', title: 'claude' }),
        makeTab({ id: 'tab-plain', title: 'Terminal 2' })
      ],
      agentTabIds: new Set(['tab-agent'])
    })

    expect(rows.map((r) => r.tabId)).toEqual(['tab-plain'])
  })

  it('prefers a non-empty custom title over the live title', () => {
    const rows = buildWorktreeTerminalRows({
      tabs: [makeTab({ id: 'tab-1', title: 'npm run dev', customTitle: 'Dev server' })],
      agentTabIds: new Set()
    })

    expect(rows[0]?.title).toBe('Dev server')
  })

  it('falls back to the default title when title is blank', () => {
    const rows = buildWorktreeTerminalRows({
      tabs: [makeTab({ id: 'tab-1', title: '', customTitle: null, defaultTitle: 'Terminal 3' })],
      agentTabIds: new Set()
    })

    expect(rows[0]?.title).toBe('Terminal 3')
  })

  it('orders rows by sortOrder then createdAt', () => {
    const rows = buildWorktreeTerminalRows({
      tabs: [
        makeTab({ id: 'c', sortOrder: 2, createdAt: 30 }),
        makeTab({ id: 'a', sortOrder: 0, createdAt: 10 }),
        makeTab({ id: 'b', sortOrder: 1, createdAt: 20 })
      ],
      agentTabIds: new Set()
    })

    expect(rows.map((r) => r.tabId)).toEqual(['a', 'b', 'c'])
  })

  it('returns nothing when every tab is an agent', () => {
    const rows = buildWorktreeTerminalRows({
      tabs: [makeTab({ id: 'tab-1' }), makeTab({ id: 'tab-2' })],
      agentTabIds: new Set(['tab-1', 'tab-2'])
    })

    expect(rows).toEqual([])
  })

  it('reports live-PTY state for the row', () => {
    const rows = buildWorktreeTerminalRows({
      tabs: [
        makeTab({ id: 'live' }),
        makeTab({ id: 'dead' })
      ],
      agentTabIds: new Set(),
      ptyIdsByTabId: { live: ['pty-1'] }
    })

    expect(rows.find((r) => r.tabId === 'live')?.hasLivePty).toBe(true)
    expect(rows.find((r) => r.tabId === 'dead')?.hasLivePty).toBe(false)
  })

  it('tolerates a missing ptyIdsByTabId map', () => {
    const rows = buildWorktreeTerminalRows({
      tabs: [makeTab({ id: 'tab-1' })],
      agentTabIds: new Set()
    })

    expect(rows[0]?.hasLivePty).toBe(false)
  })
})
