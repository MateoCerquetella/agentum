import { describe, expect, it } from 'vitest'
import {
  orderOperationalProjects,
  visibleOperationalProjectCount
} from './operational-project-overflow'

describe('visibleOperationalProjectCount', () => {
  it('reserves All and overflow controls before packing a contiguous prefix', () => {
    expect(
      visibleOperationalProjectCount({ availableWidth: 300, reservedWidth: 100, projectWidths: [70, 80, 90] })
    ).toBe(2)
  })

  it('falls back to overflow only when width is unavailable', () => {
    expect(visibleOperationalProjectCount({ availableWidth: 0, projectWidths: [70] })).toBe(0)
  })

  it('shows two compact project chips in the common 278px sidebar rail', () => {
    expect(
      visibleOperationalProjectCount({
        availableWidth: 262,
        reservedWidth: 76,
        projectWidths: [77, 88, 88]
      })
    ).toBe(2)
  })
})

describe('orderOperationalProjects', () => {
  const repos = [
    { id: 'barik', displayName: 'barik-enhanced' },
    { id: 'www', displayName: 'agentum-www' },
    { id: 'agentum', displayName: 'agentum' }
  ]

  it('prioritizes selected projects, then the active workspace project', () => {
    expect(
      orderOperationalProjects({
        repos,
        selectedRepoIds: ['www'],
        activeRepoId: 'agentum'
      }).map((repo) => repo.id)
    ).toEqual(['www', 'agentum', 'barik'])
  })

  it('uses workspace count before name when All is selected', () => {
    expect(
      orderOperationalProjects({
        repos,
        selectedRepoIds: [],
        workspaceCountByRepoId: new Map([
          ['agentum', 8],
          ['barik', 1],
          ['www', 1]
        ])
      }).map((repo) => repo.id)
    ).toEqual(['agentum', 'www', 'barik'])
  })
})
