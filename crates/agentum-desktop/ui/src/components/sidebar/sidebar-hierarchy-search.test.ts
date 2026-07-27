import { describe, expect, it } from 'vitest'
import { matchesSidebarHierarchySearch } from './sidebar-hierarchy-search'

describe('matchesSidebarHierarchySearch', () => {
  const values = [
    'Dyaus',
    'Linux · SSH',
    'agentum',
    'Sidebar density pass',
    'Codex',
    'Vite dev server'
  ]

  it('matches host, project, workspace, and session labels case-insensitively', () => {
    expect(matchesSidebarHierarchySearch('dyaus', values)).toBe(true)
    expect(matchesSidebarHierarchySearch('AGENTUM', values)).toBe(true)
    expect(matchesSidebarHierarchySearch('density', values)).toBe(true)
    expect(matchesSidebarHierarchySearch('vite', values)).toBe(true)
  })

  it('requires every search term while allowing them to come from different levels', () => {
    expect(matchesSidebarHierarchySearch('ssh sidebar', values)).toBe(true)
    expect(matchesSidebarHierarchySearch('ssh missing', values)).toBe(false)
  })

  it('treats an empty query as unfiltered', () => {
    expect(matchesSidebarHierarchySearch('   ', values)).toBe(true)
  })
})
