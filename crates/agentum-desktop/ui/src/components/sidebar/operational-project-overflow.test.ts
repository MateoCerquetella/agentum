import { describe, expect, it } from 'vitest'
import { visibleOperationalProjectCount } from './operational-project-overflow'

describe('visibleOperationalProjectCount', () => {
  it('reserves All and overflow controls before packing a contiguous prefix', () => {
    expect(
      visibleOperationalProjectCount({ availableWidth: 300, reservedWidth: 100, projectWidths: [70, 80, 90] })
    ).toBe(2)
  })

  it('falls back to overflow only when width is unavailable', () => {
    expect(visibleOperationalProjectCount({ availableWidth: 0, projectWidths: [70] })).toBe(0)
  })
})
