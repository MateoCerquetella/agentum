// Spec 009 (#361) F3: the context-missing reducer drives the pinned-chat
// warning banner. Pure model tests (repo convention — no DOM).
import { describe, expect, it } from 'vitest'

import { applyContextDelta, clearContextMissing, contextWarningText } from './chat-context-status'

describe('applyContextDelta', () => {
  it('sets on missing and clears on ok', () => {
    const m1 = applyContextDelta({}, 'c1', 'missing')
    expect(m1).toEqual({ c1: true })
    const m2 = applyContextDelta(m1, 'c1', 'ok')
    expect(m2).toEqual({})
  })

  it('is identity (same reference) when nothing changes', () => {
    const empty = {}
    expect(applyContextDelta(empty, 'c1', 'ok')).toBe(empty)
    const set = applyContextDelta(empty, 'c1', 'missing')
    expect(applyContextDelta(set, 'c1', 'missing')).toBe(set)
  })

  it('tracks conversations independently', () => {
    const m = applyContextDelta(applyContextDelta({}, 'a', 'missing'), 'b', 'missing')
    expect(applyContextDelta(m, 'a', 'ok')).toEqual({ b: true })
  })

  it('clearContextMissing mirrors the ok transition (fresh-send lifecycle)', () => {
    const set = applyContextDelta({}, 'c1', 'missing')
    expect(clearContextMissing(set, 'c1')).toEqual({})
    const empty = {}
    expect(clearContextMissing(empty, 'c1')).toBe(empty)
  })
})

describe('contextWarningText', () => {
  it('names the project when known', () => {
    expect(contextWarningText('agentum')).toContain("read agentum's files")
  })
  it('falls back to a generic subject', () => {
    expect(contextWarningText(null)).toContain("this project's files")
  })
})
