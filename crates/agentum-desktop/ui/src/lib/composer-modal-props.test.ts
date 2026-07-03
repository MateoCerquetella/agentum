import { describe, expect, it } from 'vitest'
import { initialStartGatedRunProp } from './composer-modal-props'

// Spec 008 F1 #1 (AC 1): the Tasks-page pre-armed hop must arm the composer's
// "Start gated run" toggle. Pin the modalData.startGatedRun → initialStartGatedRun
// leg so it can never silently drop the armed flag.
describe('initialStartGatedRunProp', () => {
  it('arms the toggle when modalData.startGatedRun is true', () => {
    expect(initialStartGatedRunProp({ startGatedRun: true })).toEqual({
      initialStartGatedRun: true
    })
  })

  it('leaves the prop untouched (empty spread) when the flag is absent/false', () => {
    expect(initialStartGatedRunProp({ startGatedRun: false })).toEqual({})
    expect(initialStartGatedRunProp({})).toEqual({})
    expect(initialStartGatedRunProp(null)).toEqual({})
    expect(initialStartGatedRunProp(undefined)).toEqual({})
  })
})
