import { describe, expect, it } from 'vitest'
import type { Repo } from '../../../../shared/types'
import {
  getProjectContextMenuTarget,
  shouldSuppressProjectHeaderClick
} from './project-context-menu'

function makeRepo(overrides: Partial<Repo> = {}): Repo {
  return {
    id: 'repo-1',
    path: '/tmp/repo-1',
    displayName: 'Repo One',
    badgeColor: '#fff',
    addedAt: 0,
    ...overrides
  } as Repo
}

describe('getProjectContextMenuTarget', () => {
  it('returns the cursor-anchored target for a repo row in repo grouping', () => {
    const repo = makeRepo()
    expect(
      getProjectContextMenuTarget({ groupBy: 'repo', repo, clientX: 120, clientY: 240 })
    ).toEqual({ repo, x: 120, y: 240 })
  })

  it('returns the cursor-anchored target for a project row under "host" grouping', () => {
    // Why: under By Host grouping projects render as repo sub-headers (with
    // row.repo set) nested under host headers, so right-click must work there
    // too — that is the default grouping users see.
    const repo = makeRepo()
    expect(
      getProjectContextMenuTarget({ groupBy: 'host', repo, clientX: 7, clientY: 9 })
    ).toEqual({ repo, x: 7, y: 9 })
  })

  it('returns null for groupings without per-project header rows', () => {
    const repo = makeRepo()
    expect(
      getProjectContextMenuTarget({ groupBy: 'none', repo, clientX: 1, clientY: 2 })
    ).toBeNull()
    expect(
      getProjectContextMenuTarget({ groupBy: 'workspace-status', repo, clientX: 1, clientY: 2 })
    ).toBeNull()
    expect(
      getProjectContextMenuTarget({ groupBy: 'pr-status', repo, clientX: 1, clientY: 2 })
    ).toBeNull()
  })

  it('returns null when there is no repo (group/host header row)', () => {
    expect(
      getProjectContextMenuTarget({ groupBy: 'repo', repo: null, clientX: 1, clientY: 2 })
    ).toBeNull()
    expect(
      getProjectContextMenuTarget({ groupBy: 'repo', repo: undefined, clientX: 1, clientY: 2 })
    ).toBeNull()
  })
})

describe('shouldSuppressProjectHeaderClick', () => {
  it('never suppresses when no menu open was recorded', () => {
    expect(shouldSuppressProjectHeaderClick(null, 1000)).toBe(false)
  })

  it('suppresses the click that immediately follows opening the menu', () => {
    expect(shouldSuppressProjectHeaderClick(1000, 1000)).toBe(true)
    expect(shouldSuppressProjectHeaderClick(1000, 1499)).toBe(true)
    expect(shouldSuppressProjectHeaderClick(1000, 1500)).toBe(true)
  })

  it('does not suppress once the suppression window has elapsed', () => {
    expect(shouldSuppressProjectHeaderClick(1000, 1501)).toBe(false)
  })

  it('does not suppress a click that predates the recorded open (clock skew)', () => {
    expect(shouldSuppressProjectHeaderClick(1000, 999)).toBe(false)
  })
})
