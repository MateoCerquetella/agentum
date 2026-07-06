import { describe, expect, it } from 'vitest'
import { projectsNavRows } from './projects-nav-rows'

const repos = [
  { id: 'r1', displayName: 'agentum' },
  { id: 'r2', displayName: 'agentum-tui' }
]

describe('projectsNavRows', () => {
  it('maps every repo to a row, preserving store order and display names', () => {
    const rows = projectsNavRows(repos, 'terminal', null)
    expect(rows).toEqual([
      { id: 'r1', label: 'agentum', active: false },
      { id: 'r2', label: 'agentum-tui', active: false }
    ])
  })

  it('marks a row active only on the project view WITH the matching repo', () => {
    const rows = projectsNavRows(repos, 'project', 'r2')
    expect(rows.map((r) => r.active)).toEqual([false, true])
  })

  it('keeps every row inactive on the project view when no repo matches', () => {
    const rows = projectsNavRows(repos, 'project', 'other')
    expect(rows.every((r) => !r.active)).toBe(true)
  })

  it('never marks a row active off the project view, even when activeRepoId matches', () => {
    // activeRepoId also tracks the active workspace's repo on other views —
    // highlighting a project row there would be a wrong location indicator.
    for (const view of ['terminal', 'tasks', 'activity', 'settings', 'harness']) {
      const rows = projectsNavRows(repos, view, 'r1')
      expect(rows.every((r) => !r.active)).toBe(true)
    }
  })

  it('returns no rows for an empty repo list', () => {
    expect(projectsNavRows([], 'project', 'r1')).toEqual([])
  })
})
