import { describe, expect, it } from 'vitest'

import {
  advanceIntake,
  clampStage,
  fastIntake,
  isSocraticComplete,
  normalizeIntake,
  socraticIntake,
  SOCRATIC_FINAL_STAGE,
  SOCRATIC_FIRST_STAGE
} from './socratic-intake'

// Spec 008 F2, AC 7: the staged Socratic progression is a CLIENT invariant
// ("advances exactly one pass per user turn and never skips") — pin it here on
// the pure reducer, with no React/DOM/xterm imports.
describe('socratic-intake progression (spec 008 F2, AC 7)', () => {
  it('advances exactly one pass per user turn and never skips', () => {
    let s = socraticIntake()
    expect(s).toEqual({ mode: 'socratic', stage: SOCRATIC_FIRST_STAGE })
    const seen = [s.stage]
    // Six advances from pass 1 — one per user turn, never skipping, capped at 5.
    for (let i = 0; i < 6; i++) {
      s = advanceIntake(s)
      seen.push(s.stage)
    }
    expect(seen).toEqual([1, 2, 3, 4, 5, 5, 5])
  })

  it('caps at the final pass (5) and reports completion only there', () => {
    let s = socraticIntake()
    expect(isSocraticComplete(s)).toBe(false)
    for (let i = 0; i < 10; i++) s = advanceIntake(s)
    expect(s.stage).toBe(SOCRATIC_FINAL_STAGE)
    expect(isSocraticComplete(s)).toBe(true)
    // Mid-interview is not complete.
    expect(isSocraticComplete({ mode: 'socratic', stage: 3 })).toBe(false)
  })

  it('Fast never advances and never completes', () => {
    const f = fastIntake()
    expect(advanceIntake(f)).toEqual(f)
    expect(isSocraticComplete(f)).toBe(false)
    // Even a (nonsensical) fast state with a high stage stays put.
    expect(advanceIntake({ mode: 'fast', stage: 3 })).toEqual({ mode: 'fast', stage: 3 })
  })

  it('clamps any stage into 1..=5', () => {
    expect(clampStage(0)).toBe(1)
    expect(clampStage(-4)).toBe(1)
    expect(clampStage(3)).toBe(3)
    expect(clampStage(9)).toBe(5)
    expect(clampStage(2.7)).toBe(2)
    expect(clampStage(Number.NaN)).toBe(1)
    expect(clampStage(Number.POSITIVE_INFINITY)).toBe(1)
  })

  it('normalizes an absent/legacy intake to Fast and clamps a bad stage (D1)', () => {
    // A pre-008 conversation (or a cleared store) carries no intake ⇒ restart Fast.
    expect(normalizeIntake(undefined)).toEqual(fastIntake())
    expect(normalizeIntake(null)).toEqual(fastIntake())
    expect(normalizeIntake({ mode: 'fast', stage: 9 })).toEqual(fastIntake())
    // A stored socratic thread resumes at its (clamped) pass.
    expect(normalizeIntake({ mode: 'socratic', stage: 99 })).toEqual({ mode: 'socratic', stage: 5 })
    expect(normalizeIntake({ mode: 'socratic', stage: 3 })).toEqual({ mode: 'socratic', stage: 3 })
    expect(normalizeIntake({ mode: 'socratic' })).toEqual({ mode: 'socratic', stage: SOCRATIC_FIRST_STAGE })
  })
})
