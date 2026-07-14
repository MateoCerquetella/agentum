import { describe, expect, it } from 'vitest'

import {
  isActiveProjectFor,
  resolveActiveProject,
  withActiveProjectSelection
} from './active-project-binding'
import type { GitHubProjectSettings } from './github-project-types'

const legacy = { owner: 'acme', ownerType: 'organization' as const, number: 7 }
const boardA = { owner: 'acme', ownerType: 'organization' as const, number: 1 }
const boardB = { owner: 'me', ownerType: 'user' as const, number: 2 }

function gh(overrides: Partial<GitHubProjectSettings> = {}): GitHubProjectSettings {
  return {
    pinned: [],
    recent: [],
    lastViewByProject: {},
    activeProject: null,
    ...overrides
  }
}

describe('resolveActiveProject', () => {
  it('per-repo binding wins over the legacy global', () => {
    const s = gh({ activeProject: legacy, activeProjectByRepo: { 'repo-a': boardA } })
    expect(resolveActiveProject(s, 'repo-a')).toEqual(boardA)
  })

  it('falls back to the legacy global when the repo has no binding (migration)', () => {
    const s = gh({ activeProject: legacy, activeProjectByRepo: { 'repo-a': boardA } })
    expect(resolveActiveProject(s, 'repo-b')).toEqual(legacy)
  })

  it('null repo scope (multi-repo board) resolves the legacy global', () => {
    const s = gh({ activeProject: legacy, activeProjectByRepo: { 'repo-a': boardA } })
    expect(resolveActiveProject(s, null)).toEqual(legacy)
  })

  it('nothing configured anywhere resolves to null', () => {
    expect(resolveActiveProject(gh(), 'repo-a')).toBeNull()
    expect(resolveActiveProject(undefined, 'repo-a')).toBeNull()
  })

  it('tolerates profiles persisted before the per-repo map existed', () => {
    // activeProjectByRepo absent entirely — the pre-#360 settings shape.
    expect(resolveActiveProject(gh({ activeProject: legacy }), 'repo-a')).toEqual(legacy)
  })
})

describe('withActiveProjectSelection', () => {
  it('a repo-scoped pick writes only that repo — the legacy global is untouched', () => {
    const prev = gh({ activeProject: legacy, activeProjectByRepo: { 'repo-b': boardB } })
    const next = withActiveProjectSelection(prev, 'repo-a', boardA)
    expect(next.activeProjectByRepo).toEqual({ 'repo-a': boardA, 'repo-b': boardB })
    expect(next.activeProject).toEqual(legacy)
  })

  it('picking in project A does not change project B (AC #360)', () => {
    let s = gh({ activeProjectByRepo: { 'repo-b': boardB } })
    s = withActiveProjectSelection(s, 'repo-a', boardA)
    expect(resolveActiveProject(s, 'repo-b')).toEqual(boardB)
    expect(resolveActiveProject(s, 'repo-a')).toEqual(boardA)
  })

  it('no repo scope keeps the old global persistence', () => {
    const next = withActiveProjectSelection(gh(), null, boardA)
    expect(next.activeProject).toEqual(boardA)
    expect(next.activeProjectByRepo).toBeUndefined()
  })

  it('rebinding a repo replaces its entry without dropping siblings', () => {
    const prev = gh({ activeProjectByRepo: { 'repo-a': boardA, 'repo-b': boardB } })
    const next = withActiveProjectSelection(prev, 'repo-a', legacy)
    expect(next.activeProjectByRepo).toEqual({ 'repo-a': legacy, 'repo-b': boardB })
  })
})

describe('isActiveProjectFor', () => {
  it('matches the resolved per-repo binding', () => {
    const s = gh({ activeProject: legacy, activeProjectByRepo: { 'repo-a': boardA } })
    expect(isActiveProjectFor(s, 'repo-a', boardA)).toBe(true)
    expect(isActiveProjectFor(s, 'repo-a', legacy)).toBe(false)
  })

  it('matches the legacy fallback for an unbound repo', () => {
    const s = gh({ activeProject: legacy })
    expect(isActiveProjectFor(s, 'repo-a', legacy)).toBe(true)
    expect(isActiveProjectFor(gh(), 'repo-a', legacy)).toBe(false)
  })
})
