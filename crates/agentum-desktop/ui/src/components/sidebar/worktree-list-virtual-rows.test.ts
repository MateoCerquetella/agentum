import { describe, expect, it } from 'vitest'
import { estimateRenderRowSize, type RenderRow } from './worktree-list-virtual-rows'

describe('operational virtual row estimates', () => {
  it('gives the settled disclosure control a compact stable seed height', () => {
    const rows: RenderRow[] = [
      {
        type: 'operational-settled-disclosure',
        key: 'operational:settled:disclosure',
        remainingCount: 4,
        expanded: false
      }
    ]
    expect(estimateRenderRowSize(rows, 0, -1, null)).toBe(34)
  })
})
