import { describe, expect, it } from 'vitest'
import { clipToOverlayRect, pointToDevice } from './agent-browser-picker'

describe('agent-browser-picker geometry', () => {
  // The pane sets the page viewport to the canvas CSS box, so in steady state
  // deviceWidth/Height equal the canvas box size and the map is identity.
  it('maps a client point to device coords at unit scale', () => {
    const rect = { left: 0, top: 0, width: 900, height: 600 }
    expect(pointToDevice(450, 300, rect, 900, 600)).toEqual({ x: 450, y: 300 })
  })

  it('subtracts the canvas top-left offset', () => {
    const rect = { left: 10, top: 10, width: 900, height: 600 }
    expect(pointToDevice(110, 60, rect, 900, 600)).toEqual({ x: 100, y: 50 })
  })

  it('round-trips a device point through clipToOverlayRect at unit scale', () => {
    const rect = { left: 0, top: 0, width: 900, height: 600 }
    const p = pointToDevice(120, 80, rect, 900, 600)!
    const overlay = clipToOverlayRect({ x: p.x, y: p.y, width: 0, height: 0 }, rect, 900, 600)
    expect(overlay).toMatchObject({ left: 120, top: 80 })
  })

  it('scales an element clip when the canvas box differs from the device size', () => {
    // Canvas displayed at half the device CSS size (e.g. a resize the frame hasn't caught up to).
    const rect = { left: 0, top: 0, width: 450, height: 300 }
    expect(clipToOverlayRect({ x: 100, y: 50, width: 80, height: 40 }, rect, 900, 600)).toEqual({
      left: 50,
      top: 25,
      width: 40,
      height: 20
    })
  })

  it('returns null for a degenerate box', () => {
    expect(pointToDevice(0, 0, { left: 0, top: 0, width: 0, height: 0 }, 900, 600)).toBeNull()
    expect(
      clipToOverlayRect({ x: 0, y: 0, width: 1, height: 1 }, { left: 0, top: 0, width: 900, height: 600 }, 0, 600)
    ).toBeNull()
  })
})
