import { describe, expect, it } from 'vitest'

import { projectsPageRows } from './projects-page-rows'

const repo = (id: string, over: Partial<Parameters<typeof projectsPageRows>[0][number]> = {}) => ({
  id,
  displayName: id.toUpperCase(),
  path: `/Users/dev/${id}`,
  ...over
})

describe('projectsPageRows', () => {
  it('maps one card per repo in store order', () => {
    const rows = projectsPageRows([repo('a'), repo('b')], {})
    expect(rows.map((r) => r.id)).toEqual(['a', 'b'])
    expect(rows[0]).toEqual({
      id: 'a',
      name: 'A',
      path: '/Users/dev/a',
      remote: false,
      worktrees: 0
    })
  })

  it('flags SSH repos as remote (null connectionId stays local)', () => {
    const rows = projectsPageRows(
      [repo('local', { connectionId: null }), repo('vps', { connectionId: 'ssh-1' })],
      {}
    )
    expect(rows.map((r) => r.remote)).toEqual([false, true])
  })

  it('carries the workspace count, defaulting to 0 for unknown repos', () => {
    const rows = projectsPageRows([repo('a'), repo('b')], { a: 3 })
    expect(rows.map((r) => r.worktrees)).toEqual([3, 0])
  })

  it('empty repos ⇒ empty rows (the page shows its empty state instead)', () => {
    expect(projectsPageRows([], {})).toEqual([])
  })
})
