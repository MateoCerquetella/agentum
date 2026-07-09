// jsdom-free vitest for the spec-014 F3 pure coalescer (AC 7): N tracker
// events inside the window ⇒ exactly ONE fire; no timers needed — the model
// is a reducer over nowMs.
import { describe, expect, it } from 'vitest'
import {
  PROJECT_VIEW_EVENT_REFETCH_COALESCE_MS,
  coalesceEvent,
  coalesceFire,
  initialCoalesceState,
  isTrackerEventKind,
  type CoalesceState
} from './project-view-live-refresh'

describe('isTrackerEventKind', () => {
  it('accepts both tracker kinds and rejects everything else', () => {
    expect(isTrackerEventKind('tracker.phase_changed')).toBe(true)
    expect(isTrackerEventKind('tracker.blocked')).toBe(true)
    expect(isTrackerEventKind('agent.working')).toBe(false)
    expect(isTrackerEventKind('session.started')).toBe(false)
    expect(isTrackerEventKind(undefined)).toBe(false)
    expect(isTrackerEventKind(42)).toBe(false)
  })
})

describe('coalesce reducer', () => {
  it('a burst of events inside the window schedules exactly one fire', () => {
    let state: CoalesceState = initialCoalesceState
    let scheduled = 0
    // Five events spread over the 2s window.
    for (const t of [0, 100, 500, 1500, 1999]) {
      const next = coalesceEvent(state, t)
      state = next.state
      if (next.schedule) {
        scheduled += 1
      }
    }
    expect(scheduled).toBe(1)
    // The one scheduled fire lands at window close; the fire closes the window.
    state = coalesceFire(state)
    expect(state.windowOpenedAtMs).toBeNull()
  })

  it('an event after the window fired opens a NEW window (second fetch)', () => {
    let state: CoalesceState = initialCoalesceState
    let scheduled = 0
    const step = (t: number): void => {
      const next = coalesceEvent(state, t)
      state = next.state
      if (next.schedule) {
        scheduled += 1
      }
    }
    step(0)
    step(1000)
    // The scheduled fire runs at t = 0 + window.
    state = coalesceFire(state)
    // A later event, after the window closed, schedules again.
    step(PROJECT_VIEW_EVENT_REFETCH_COALESCE_MS + 500)
    expect(scheduled).toBe(2)
  })

  it('firing with no open window is a no-op (stale timer safety)', () => {
    expect(coalesceFire(initialCoalesceState)).toBe(initialCoalesceState)
  })

  it('the window length is the named 2s constant', () => {
    expect(PROJECT_VIEW_EVENT_REFETCH_COALESCE_MS).toBe(2_000)
  })
})
