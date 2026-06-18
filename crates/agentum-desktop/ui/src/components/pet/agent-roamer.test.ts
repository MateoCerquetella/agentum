import { describe, expect, it } from 'vitest'
import {
  ROAMER_POSE,
  advanceRoamer,
  createRoamerState,
  happyHop,
  roamerPose,
  type RoamerEnv,
  type RoamerState
} from './agent-roamer'

const env: RoamerEnv = { size: 100, groundY: 500, minX: 0, maxX: 600 }
const zeroRng = (): number => 0

// A grounded, idle-but-seeking base state we can steer in tests.
function grounded(overrides: Partial<RoamerState> = {}): RoamerState {
  const s = createRoamerState(env, zeroRng)
  return { ...s, jz: 0, jv: 0, idleT: 0, blinkT: 5, nextTripT: 999, ...overrides }
}

describe('agent-roamer', () => {
  it('walks toward a target to its right and faces that way', () => {
    let s = grounded({ x: 0, targetX: 300 })
    for (let i = 0; i < 30; i++) {
      s = advanceRoamer(s, env, 0.1, zeroRng)
    }
    expect(s.x).toBeGreaterThan(20)
    expect(s.dir).toBe(1)
    expect(s.moving).toBe(true)
  })

  it('stops and idles once it reaches the target', () => {
    let s = grounded({ x: 298, targetX: 300 })
    s = advanceRoamer(s, env, 0.1, zeroRng) // within 6px → start idling
    expect(s.idleT).toBeGreaterThan(0)
    // A few frames inside the idle window: still resting, not walking.
    for (let i = 0; i < 3; i++) {
      s = advanceRoamer(s, env, 0.1, zeroRng)
    }
    expect(s.moving).toBe(false)
    expect(Math.abs(s.vx)).toBeLessThan(5)
  })

  it('falls back to the floor after a hop (gravity)', () => {
    let s = happyHop(grounded({ x: 100, targetX: 100 }), env.size)
    expect(s.jz).toBeLessThan(0) // airborne immediately after the hop
    let wentUp = false
    for (let i = 0; i < 240; i++) {
      s = advanceRoamer(s, env, 1 / 60, zeroRng)
      if (s.jz < -1) {
        wentUp = true
      }
    }
    expect(wentUp).toBe(true)
    expect(s.jz).toBe(0) // landed
  })

  it('shows the jump pose while airborne and the neutral happy face at rest', () => {
    const air = grounded({ jz: -20 })
    expect(roamerPose(air)).toBe(ROAMER_POSE.jump)
    const rest = grounded({ moving: false, blinking: false })
    expect(roamerPose(rest)).toBe(ROAMER_POSE.happy)
    const blinking = grounded({ moving: false, blinking: true })
    expect(roamerPose(blinking)).toBe(ROAMER_POSE.blink)
  })

  it('runs the slip → fallen → recover trip and surfaces every pose', () => {
    // Force a trip by making it due while standing idle.
    let s = grounded({ moving: false, idleT: 1, jz: 0, nextTripT: 0, dir: 1 })
    s = advanceRoamer(s, env, 1 / 60, zeroRng)
    expect(s.state).toBe('slip')
    expect(roamerPose(s)).toBe(ROAMER_POSE.slip)
    // Through the slip window into fallen.
    for (let i = 0; i < 60; i++) {
      s = advanceRoamer(s, env, 1 / 60, zeroRng)
    }
    expect(s.state).toBe('fallen')
    expect(roamerPose(s)).toBe(ROAMER_POSE.fallen)
    // Wait out the fall, then it gets up and returns to roaming.
    for (let i = 0; i < 60 * 4; i++) {
      s = advanceRoamer(s, env, 1 / 60, zeroRng)
    }
    expect(s.state).toBe('ok')
  })

  it('stays within the floor span', () => {
    let s = grounded({ x: 0, targetX: 9999 })
    for (let i = 0; i < 400; i++) {
      s = advanceRoamer(s, env, 0.05, zeroRng)
    }
    expect(s.x).toBeLessThanOrEqual(env.maxX)
    expect(s.x).toBeGreaterThanOrEqual(env.minX)
  })

  it('does not move while being dragged', () => {
    const s = grounded({ x: 123, dragging: true, vx: 50, targetX: 600 })
    const next = advanceRoamer(s, env, 0.1, zeroRng)
    expect(next.x).toBe(123)
  })
})
