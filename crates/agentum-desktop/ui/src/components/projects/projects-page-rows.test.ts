import { describe, expect, it } from 'vitest'

import {
  filterProjectsRows,
  groupProjectsRowsByHost,
  projectsPageRows,
  type ProjectsPageRow
} from './projects-page-rows'

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
      host: 'Local',
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

  describe('host label', () => {
    it("local repos group under 'Local'", () => {
      const rows = projectsPageRows([repo('a', { connectionId: null })], {})
      expect(rows[0].host).toBe('Local')
    })

    it('remote repos resolve their host label from the ssh target labels', () => {
      const labels = new Map([['ssh-1', 'prod vps']])
      const rows = projectsPageRows([repo('vps', { connectionId: 'ssh-1' })], {}, labels)
      expect(rows[0].host).toBe('prod vps')
    })

    it("remote repos fall back to 'Remote host' when the label is unknown", () => {
      const rows = projectsPageRows([repo('vps', { connectionId: 'ssh-x' })], {}, new Map())
      expect(rows[0].host).toBe('Remote host')
    })
  })
})

const row = (name: string, host: string): ProjectsPageRow => ({
  id: name,
  name,
  path: `/Users/dev/${name}`,
  remote: host !== 'Local',
  host,
  worktrees: 0
})

describe('filterProjectsRows', () => {
  const rows = [row('agentum', 'Local'), row('Website', 'Local'), row('api', 'vps')]

  it('is a no-op for a blank query (returns a copy of every row)', () => {
    const out = filterProjectsRows(rows, '   ')
    expect(out.map((r) => r.name)).toEqual(['agentum', 'Website', 'api'])
    expect(out).not.toBe(rows)
  })

  it('matches a case-insensitive substring of the name', () => {
    expect(filterProjectsRows(rows, 'WEB').map((r) => r.name)).toEqual(['Website'])
    expect(filterProjectsRows(rows, 'a').map((r) => r.name)).toEqual(['agentum', 'api'])
  })

  it('returns nothing when no name matches', () => {
    expect(filterProjectsRows(rows, 'zzz')).toEqual([])
  })
})

describe('groupProjectsRowsByHost', () => {
  it('floats Local first, then remotes in first-seen order', () => {
    const rows = [row('api', 'vps'), row('agentum', 'Local'), row('edge', 'staging')]
    const groups = groupProjectsRowsByHost(rows)
    expect(groups.map((g) => g.host)).toEqual(['Local', 'vps', 'staging'])
    expect(groups[0].rows.map((r) => r.name)).toEqual(['agentum'])
  })

  it('omits a host entirely when it has no rows (no empty headers)', () => {
    const rows = [row('agentum', 'Local'), row('api', 'vps')]
    const filtered = filterProjectsRows(rows, 'agentum')
    const groups = groupProjectsRowsByHost(filtered)
    expect(groups.map((g) => g.host)).toEqual(['Local'])
  })

  it('empty input ⇒ no groups', () => {
    expect(groupProjectsRowsByHost([])).toEqual([])
  })
})
