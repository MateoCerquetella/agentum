import { describe, expect, it } from 'vitest'
import { clampDialogOffset } from './movable-dialog'

const baseRect = {
  left: 200,
  top: 100,
  right: 800,
  bottom: 600
}

describe('clampDialogOffset', () => {
  it('keeps an in-bounds drag unchanged', () => {
    expect(
      clampDialogOffset({
        desiredOffset: { x: 40, y: -30 },
        baseRect,
        viewportWidth: 1000,
        viewportHeight: 700
      })
    ).toEqual({ x: 40, y: -30 })
  })

  it('keeps every edge inside the viewport gutter', () => {
    expect(
      clampDialogOffset({
        desiredOffset: { x: -500, y: 500 },
        baseRect,
        viewportWidth: 1000,
        viewportHeight: 700
      })
    ).toEqual({ x: -184, y: 84 })
  })

  it('uses a custom gutter when supplied', () => {
    expect(
      clampDialogOffset({
        desiredOffset: { x: 500, y: -500 },
        baseRect,
        viewportWidth: 1000,
        viewportHeight: 700,
        gutter: 24
      })
    ).toEqual({ x: 176, y: -76 })
  })
})
