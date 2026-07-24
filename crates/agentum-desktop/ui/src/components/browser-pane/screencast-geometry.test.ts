import { describe, expect, it } from 'vitest'
import { containContentBox, clientToDevicePoint } from './screencast-geometry'

describe('containContentBox', () => {
  it('no letterbox when aspect matches — content fills the box', () => {
    expect(containContentBox(800, 600, 1600, 1200)).toEqual({
      offsetX: 0,
      offsetY: 0,
      width: 800,
      height: 600
    })
  })

  it('bars top/bottom when the frame is wider than the box', () => {
    // box 800x600 (4:3), frame 1600x600 (8:3) → fit by width, 300 tall, centered.
    expect(containContentBox(800, 600, 1600, 600)).toEqual({
      offsetX: 0,
      offsetY: 150,
      width: 800,
      height: 300
    })
  })

  it('bars left/right when the frame is taller than the box', () => {
    // box 800x600, frame 600x1200 (1:2) → fit by height, 300 wide, centered.
    expect(containContentBox(800, 600, 600, 1200)).toEqual({
      offsetX: 250,
      offsetY: 0,
      width: 300,
      height: 600
    })
  })

  it('returns null for a degenerate box or frame', () => {
    expect(containContentBox(0, 600, 100, 100)).toBeNull()
    expect(containContentBox(800, 600, 0, 100)).toBeNull()
  })
})

describe('clientToDevicePoint', () => {
  it('maps exactly when there is no letterbox', () => {
    // box == frame aspect (2× scale): center of box → center of frame.
    expect(clientToDevicePoint(400, 300, 800, 600, 1600, 1200)).toEqual({ x: 800, y: 600 })
    expect(clientToDevicePoint(0, 0, 800, 600, 1600, 1200)).toEqual({ x: 0, y: 0 })
  })

  it('accounts for top/bottom bars (offset + scale) so a click hits the right pixel', () => {
    // box 800x600, frame 1600x600 → content is y∈[150,450], 800x300 painted.
    // A click at the top edge of the PAINTED image (y=150) maps to frame y=0.
    expect(clientToDevicePoint(400, 150, 800, 600, 1600, 600)).toEqual({ x: 800, y: 0 })
    // Middle of the painted image → middle of the frame.
    expect(clientToDevicePoint(400, 300, 800, 600, 1600, 600)).toEqual({ x: 800, y: 300 })
  })

  it('drops a click that lands on a letterbox bar (never mis-routed)', () => {
    // y=50 is inside the top black bar (bars are y<150) → no page target.
    expect(clientToDevicePoint(400, 50, 800, 600, 1600, 600)).toBeNull()
  })

  it('returns null for a degenerate box', () => {
    expect(clientToDevicePoint(10, 10, 0, 0, 100, 100)).toBeNull()
  })
})
