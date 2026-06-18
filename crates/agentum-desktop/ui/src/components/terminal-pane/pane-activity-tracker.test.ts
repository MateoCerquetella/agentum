import { describe, expect, it } from 'vitest'
import { createPaneActivityTracker } from './pane-activity-tracker'

/** Manual timer queue so idle transitions are deterministic in tests. */
function makeClock() {
  let nextId = 1
  const pending = new Map<number, { fn: () => void; due: number }>()
  let nowMs = 0
  return {
    setTimer: (fn: () => void, ms: number) => {
      const id = nextId++
      pending.set(id, { fn, due: nowMs + ms })
      return id as unknown as ReturnType<typeof setTimeout>
    },
    clearTimer: (handle: ReturnType<typeof setTimeout>) => {
      pending.delete(handle as unknown as number)
    },
    /** Advance the clock, firing any timers that come due. */
    advance: (ms: number) => {
      nowMs += ms
      for (const [id, t] of [...pending.entries()]) {
        if (t.due <= nowMs) {
          pending.delete(id)
          t.fn()
        }
      }
    }
  }
}

describe('createPaneActivityTracker', () => {
  it('reports working on first activity, only once until idle', () => {
    const clock = makeClock()
    let working = 0
    let idle = 0
    const tracker = createPaneActivityTracker({
      idleAfterMs: 3000,
      onWorking: () => (working += 1),
      onIdle: () => (idle += 1),
      setTimer: clock.setTimer,
      clearTimer: clock.clearTimer
    })

    tracker.noteActivity()
    tracker.noteActivity()
    tracker.noteActivity()

    expect(working).toBe(1)
    expect(idle).toBe(0)
    expect(tracker.state()).toBe('working')
  })

  it('reports idle after a quiet window, then working again on new activity', () => {
    const clock = makeClock()
    let working = 0
    let idle = 0
    const tracker = createPaneActivityTracker({
      idleAfterMs: 3000,
      onWorking: () => (working += 1),
      onIdle: () => (idle += 1),
      setTimer: clock.setTimer,
      clearTimer: clock.clearTimer
    })

    tracker.noteActivity()
    clock.advance(3000)
    expect(idle).toBe(1)
    expect(tracker.state()).toBe('idle')

    tracker.noteActivity()
    expect(working).toBe(2)
    expect(tracker.state()).toBe('working')
  })

  it('keeps resetting the idle timer while activity continues', () => {
    const clock = makeClock()
    let idle = 0
    const tracker = createPaneActivityTracker({
      idleAfterMs: 3000,
      onWorking: () => {},
      onIdle: () => (idle += 1),
      setTimer: clock.setTimer,
      clearTimer: clock.clearTimer
    })

    tracker.noteActivity()
    clock.advance(2000)
    tracker.noteActivity() // resets the window
    clock.advance(2000)
    expect(idle).toBe(0) // never went quiet for a full 3s
    clock.advance(1000)
    expect(idle).toBe(1)
  })

  it('fires no callbacks after dispose', () => {
    const clock = makeClock()
    let idle = 0
    const tracker = createPaneActivityTracker({
      idleAfterMs: 3000,
      onWorking: () => {},
      onIdle: () => (idle += 1),
      setTimer: clock.setTimer,
      clearTimer: clock.clearTimer
    })

    tracker.noteActivity()
    tracker.dispose()
    clock.advance(5000)
    expect(idle).toBe(0)
  })
})
