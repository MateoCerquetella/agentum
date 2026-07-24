import { describe, expect, it } from 'vitest'

import {
  advanceIntake,
  clampStage,
  fastIntake,
  isSocraticComplete,
  normalizeIntake,
  parseSocraticControl,
  resolveIntakeAfterReply,
  socraticIntake,
  SOCRATIC_FINAL_STAGE,
  SOCRATIC_FIRST_STAGE,
  stripSocraticControl
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
    // #257: an explicit converged flag survives normalization; junk doesn't.
    expect(normalizeIntake({ mode: 'socratic', stage: 5, converged: true })).toEqual({
      mode: 'socratic',
      stage: 5,
      converged: true
    })
  })
})

// #257 — the adaptive progression: the model's trailing control marker moves
// the stage machine (stay re-runs a pass, done converges), with a marker-less
// reply falling back to the legacy one-pass advance.
describe('socratic-intake adaptive control markers (#257)', () => {
  it('parses the trailing control marker (and only a trailing one)', () => {
    expect(parseSocraticControl('Sharp question?\n\n[[socratic:advance]]')).toBe('advance')
    expect(parseSocraticControl('Still vague — who exactly?\n[[socratic:stay]]\n')).toBe('stay')
    expect(parseSocraticControl('Spec is defined.\n[[ socratic : DONE ]]')).toBe('done')
    expect(parseSocraticControl('No marker here.')).toBeNull()
    // Mid-text mention is not a control (the marker must terminate the reply).
    expect(parseSocraticControl('mentions [[socratic:advance]] early\nthen more text')).toBeNull()
  })

  it('strips the marker from the transcript and leaves clean text untouched', () => {
    expect(stripSocraticControl('Ask away.\n\n[[socratic:advance]]')).toBe('Ask away.')
    expect(stripSocraticControl('Plain reply.')).toBe('Plain reply.')
  })

  it('stay re-runs the pass, advance steps one, done converges', () => {
    const atThree = { mode: 'socratic', stage: 3 } as const
    expect(resolveIntakeAfterReply(atThree, 'more depth needed\n[[socratic:stay]]')).toEqual({
      mode: 'socratic',
      stage: 3
    })
    expect(resolveIntakeAfterReply(atThree, 'covered\n[[socratic:advance]]')).toEqual({
      mode: 'socratic',
      stage: 4
    })
    expect(resolveIntakeAfterReply(atThree, 'spec is ready\n[[socratic:done]]')).toEqual({
      mode: 'socratic',
      stage: SOCRATIC_FINAL_STAGE,
      converged: true
    })
  })

  it('falls back to the legacy one-pass advance when no marker is present', () => {
    expect(resolveIntakeAfterReply({ mode: 'socratic', stage: 2 }, 'old-server reply')).toEqual({
      mode: 'socratic',
      stage: 3
    })
  })

  it('never moves a Fast thread', () => {
    const fast = fastIntake()
    expect(resolveIntakeAfterReply(fast, 'anything\n[[socratic:advance]]')).toEqual(fast)
  })

  it('completion is marker-driven once a converged flag exists', () => {
    // done → converged, even mid-stage-count semantics.
    expect(isSocraticComplete({ mode: 'socratic', stage: 5, converged: true })).toBe(true)
    // an explicit not-yet-converged final pass keeps interviewing.
    expect(isSocraticComplete({ mode: 'socratic', stage: 5, converged: false })).toBe(false)
    // legacy state without the flag keeps the old reached-final-pass rule.
    expect(isSocraticComplete({ mode: 'socratic', stage: 5 })).toBe(true)
  })
})
