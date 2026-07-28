import { describe, it, expect } from 'vitest'
import { latestFromEntries } from './worktree-latest-activity'
import type { AgentStatusEntry } from '@/shared/agent-status-types'

function entry(over: Partial<AgentStatusEntry>): AgentStatusEntry {
  return {
    state: 'working',
    prompt: '',
    updatedAt: 0,
    stateStartedAt: 0,
    paneKey: 'tab:leaf',
    stateHistory: [],
    ...over
  }
}

describe('latestFromEntries', () => {
  it('returns empty fields for no entries', () => {
    expect(latestFromEntries([])).toEqual({})
  })

  it('picks the entry with the greatest updatedAt', () => {
    const result = latestFromEntries([
      entry({ updatedAt: 10, lastAssistantMessage: 'old', toolName: 'Read' }),
      entry({ updatedAt: 30, lastAssistantMessage: 'Wired the worktree help', toolName: 'Bash', toolInput: 'cargo clippy', contextUsagePercent: 71 }),
      entry({ updatedAt: 20, lastAssistantMessage: 'mid' })
    ])
    expect(result).toEqual({
      lastAssistantMessage: 'Wired the worktree help',
      toolName: 'Bash',
      toolInput: 'cargo clippy',
      contextUsagePercent: 71
    })
  })
})
