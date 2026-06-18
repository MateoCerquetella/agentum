import { describe, expect, it } from 'vitest'
import { stashPendingSessionPrompt, takePendingSessionPrompt } from './pending-session-prompt'

describe('pending-session-prompt', () => {
  it('hands a stashed prompt back once, then clears it (read-once)', () => {
    stashPendingSessionPrompt('wt-1', 'implement feature X')
    expect(takePendingSessionPrompt('wt-1')).toBe('implement feature X')
    // Second read is empty — the picker must not replay a stale prompt.
    expect(takePendingSessionPrompt('wt-1')).toBeUndefined()
  })

  it('trims surrounding whitespace before stashing', () => {
    stashPendingSessionPrompt('wt-2', '  hello  ')
    expect(takePendingSessionPrompt('wt-2')).toBe('hello')
  })

  it('ignores a whitespace-only prompt (nothing to deliver)', () => {
    stashPendingSessionPrompt('wt-3', '   ')
    expect(takePendingSessionPrompt('wt-3')).toBeUndefined()
  })

  it('a whitespace-only restash clears a previously stashed prompt', () => {
    stashPendingSessionPrompt('wt-4', 'first')
    stashPendingSessionPrompt('wt-4', '   ')
    expect(takePendingSessionPrompt('wt-4')).toBeUndefined()
  })

  it('keeps prompts isolated per worktree id', () => {
    stashPendingSessionPrompt('wt-a', 'A')
    stashPendingSessionPrompt('wt-b', 'B')
    expect(takePendingSessionPrompt('wt-b')).toBe('B')
    expect(takePendingSessionPrompt('wt-a')).toBe('A')
  })
})
