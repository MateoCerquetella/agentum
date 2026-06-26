import { describe, expect, it } from 'vitest'
import type { AgentStatusEntry } from '../../../../shared/agent-status-types'
import { countActivityUnread } from './useActivityUnreadCount'

// Minimal entry covering only the fields countActivityUnread reads in
// 'sidebar-badge' mode (state, stateStartedAt, paneKey). Cast through unknown so
// the test does not depend on the full AgentStatusEntry shape.
function doneEntry(paneKey: string, stateStartedAt: number): AgentStatusEntry {
  return {
    paneKey,
    state: 'done',
    stateStartedAt,
    stateHistory: []
  } as unknown as AgentStatusEntry
}

const EMPTY = {
  acknowledgedAgentsByPaneKey: {},
  agentStatusByPaneKey: {},
  migrationUnsupportedByPtyId: {},
  retainedAgentsByPaneKey: {},
  worktreesByRepo: {}
}

describe('countActivityUnread (badge survives feed deletion)', () => {
  it('counts an unacknowledged done agent from store state alone', () => {
    const source = {
      ...EMPTY,
      agentStatusByPaneKey: { 'tab-1:leaf-1': doneEntry('tab-1:leaf-1', 1000) }
    }
    expect(countActivityUnread(source, 'sidebar-badge')).toBe(1)
  })

  it('does not count it once acknowledged after the state started', () => {
    const source = {
      ...EMPTY,
      agentStatusByPaneKey: { 'tab-1:leaf-1': doneEntry('tab-1:leaf-1', 1000) },
      acknowledgedAgentsByPaneKey: { 'tab-1:leaf-1': 2000 }
    }
    expect(countActivityUnread(source, 'sidebar-badge')).toBe(0)
  })

  it('returns 0 for an empty store', () => {
    expect(countActivityUnread(EMPTY, 'sidebar-badge')).toBe(0)
  })
})
