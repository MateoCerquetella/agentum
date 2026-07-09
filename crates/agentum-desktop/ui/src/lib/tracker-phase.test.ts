// jsdom-free vitest for the spec-014 tracker-phase pure model (AC 5/6/11).
import { describe, expect, it } from 'vitest'
import {
  deriveTrackerChip,
  matchEventToWorktree,
  parseTrackerPhaseWire,
  trackerEventFromFrame,
  type TrackerLiveEvent
} from './tracker-phase'

describe('parseTrackerPhaseWire', () => {
  it('accepts the five wire phases and rejects junk', () => {
    for (const phase of ['todo', 'in_progress', 'in_review', 'ready_to_test', 'done']) {
      expect(parseTrackerPhaseWire(phase)).toBe(phase)
    }
    for (const junk of ['', 'Todo', 'DONE', 'in-review', 'blocked', 42, null, undefined, {}]) {
      expect(parseTrackerPhaseWire(junk)).toBeNull()
    }
  })
})

describe('trackerEventFromFrame', () => {
  it('distills tracker.phase_changed with the full payload', () => {
    const evt = trackerEventFromFrame({
      kind: 'tracker.phase_changed',
      payload: {
        worktree_id: 'r1::/p',
        provider: 'github',
        phase: 'in_progress',
        tracker_url: 'https://github.com/o/r/issues/42'
      }
    })
    expect(evt).toEqual({
      kind: 'phase',
      worktreeId: 'r1::/p',
      trackerUrl: 'https://github.com/o/r/issues/42',
      phase: 'in_progress'
    })
  })

  it('distills tracker.blocked (no phase in the payload)', () => {
    const evt = trackerEventFromFrame({
      kind: 'tracker.blocked',
      payload: {
        worktree_id: null,
        provider: 'github',
        tracker_url: 'https://github.com/o/r/issues/42',
        reason: 'session crash'
      }
    })
    expect(evt).toEqual({
      kind: 'blocked',
      worktreeId: null,
      trackerUrl: 'https://github.com/o/r/issues/42'
    })
  })

  it('returns null for other kinds and malformed frames', () => {
    expect(trackerEventFromFrame({ kind: 'agent.working', payload: {} })).toBeNull()
    expect(trackerEventFromFrame({ kind: 'session.started' })).toBeNull()
    expect(trackerEventFromFrame({})).toBeNull()
    // A phase_changed with junk/missing phase never becomes a chip update.
    expect(
      trackerEventFromFrame({ kind: 'tracker.phase_changed', payload: { phase: 'garbage' } })
    ).toBeNull()
    expect(trackerEventFromFrame({ kind: 'tracker.phase_changed', payload: null })).toBeNull()
  })
})

describe('matchEventToWorktree', () => {
  const rows = [
    { id: 'r1::/a', trackerUrl: 'https://github.com/o/r/issues/1' },
    { id: 'r1::/b', trackerUrl: null },
    { id: 'r2::/c', trackerUrl: 'https://github.com/o/r/issues/3' }
  ]

  it('joins on worktree_id first', () => {
    const evt: TrackerLiveEvent = {
      kind: 'phase',
      worktreeId: 'r1::/b',
      trackerUrl: 'https://github.com/o/r/issues/1',
      phase: 'done'
    }
    expect(matchEventToWorktree(evt, rows)).toBe('r1::/b')
  })

  it('falls back to trackerUrl when worktree_id is null (harness/MCP emitters)', () => {
    const evt: TrackerLiveEvent = {
      kind: 'blocked',
      worktreeId: null,
      trackerUrl: 'https://github.com/o/r/issues/3'
    }
    expect(matchEventToWorktree(evt, rows)).toBe('r2::/c')
  })

  it('falls back to trackerUrl when the id misses the row set', () => {
    const evt: TrackerLiveEvent = {
      kind: 'phase',
      worktreeId: 'gone::/x',
      trackerUrl: 'https://github.com/o/r/issues/1',
      phase: 'todo'
    }
    expect(matchEventToWorktree(evt, rows)).toBe('r1::/a')
  })

  it('returns null on no match — never a fabricated join', () => {
    const evt: TrackerLiveEvent = {
      kind: 'phase',
      worktreeId: null,
      trackerUrl: 'https://github.com/o/r/issues/999',
      phase: 'todo'
    }
    expect(matchEventToWorktree(evt, rows)).toBeNull()
    expect(matchEventToWorktree({ kind: 'blocked', worktreeId: null, trackerUrl: null }, rows)).toBeNull()
  })
})

describe('deriveTrackerChip', () => {
  it('renders nothing for an unbound worktree (AC 6)', () => {
    expect(deriveTrackerChip(null, undefined)).toBeNull()
    expect(deriveTrackerChip(undefined, undefined)).toBeNull()
    // An unparseable persisted phase is never a fabricated chip.
    expect(deriveTrackerChip('garbage', undefined)).toBeNull()
  })

  it('shows the persisted phase cold (AC 4)', () => {
    expect(deriveTrackerChip('in_progress', undefined)).toEqual({
      phase: 'in_progress',
      label: 'In Progress',
      attention: false
    })
    expect(deriveTrackerChip('ready_to_test', undefined)?.label).toBe('Ready to Test')
  })

  it('lets the live event overlay win over a stale persisted phase (AC 5)', () => {
    expect(deriveTrackerChip('todo', { phase: 'in_review', attention: false })).toEqual({
      phase: 'in_review',
      label: 'In Review',
      attention: false
    })
  })

  it('blocked ⇒ attention variant; phase_changed clears it (AC 11)', () => {
    // Blocked with a known phase: keep the phase, flag attention.
    expect(deriveTrackerChip('in_progress', { attention: true })).toEqual({
      phase: 'in_progress',
      label: 'In Progress',
      attention: true
    })
    // Blocked before any persisted phase arrived: attention still surfaces
    // from the event payload alone (no follow-up fetch).
    expect(deriveTrackerChip(null, { attention: true })).toEqual({
      phase: null,
      label: 'Blocked',
      attention: true
    })
    // The clear is a real phase re-apply → a phase event with attention off.
    expect(deriveTrackerChip('in_progress', { phase: 'in_progress', attention: false })).toEqual({
      phase: 'in_progress',
      label: 'In Progress',
      attention: false
    })
  })
})
