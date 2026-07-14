import { describe, expect, it } from 'vitest'
import type { GitHubProjectSettings } from '@/shared/github-project-types'
import type { BoardBindingIdentity, BoardBindingState } from './board-project-resolution'
import { applyBoardPick, clearBoardPick, resolveBoardProject } from './board-project-resolution'

const PICK = { owner: 'acme', ownerType: 'organization' as const, number: 7 }
const BOUND = { owner: 'acme', ownerType: 'organization' as const, number: 9 }
const LEGACY = { owner: 'legacy-owner', ownerType: 'user' as const, number: 3 }

function loadedBinding(overrides?: Partial<BoardBindingIdentity>): BoardBindingState {
  return {
    status: 'loaded',
    binding: {
      projectOwner: BOUND.owner,
      projectOwnerType: BOUND.ownerType,
      projectNumber: BOUND.number,
      projectTitle: 'Bound Board',
      ...overrides
    }
  }
}

const LOADED_NULL: BoardBindingState = { status: 'loaded', binding: null }
const LOADING: BoardBindingState = { status: 'loading' }

function settings(overrides?: Partial<GitHubProjectSettings>): GitHubProjectSettings {
  return {
    pinned: [],
    recent: [],
    lastViewByProject: {},
    activeProject: null,
    activeProjectByRepo: {},
    ...overrides
  }
}

describe('resolveBoardProject', () => {
  // Case 1: pick beats binding beats legacy.
  it('pick beats binding beats legacy', () => {
    const res = resolveBoardProject({
      repoId: 'repo-A',
      settings: settings({ activeProject: LEGACY, activeProjectByRepo: { 'repo-A': PICK } }),
      bindingState: loadedBinding()
    })
    expect(res.source).toBe('pick')
    expect(res.project).toEqual(PICK)
  })

  // Case 2: binding beats legacy.
  it('binding beats legacy when there is no pick', () => {
    const res = resolveBoardProject({
      repoId: 'repo-A',
      settings: settings({ activeProject: LEGACY }),
      bindingState: loadedBinding()
    })
    expect(res).toEqual({ source: 'binding', project: BOUND })
  })

  // Case 3: legacy fallback (AC 4).
  it('falls back to the legacy global slot with no pick and a null binding', () => {
    const res = resolveBoardProject({
      repoId: 'repo-A',
      settings: settings({ activeProject: LEGACY }),
      bindingState: LOADED_NULL
    })
    expect(res).toEqual({ source: 'legacy', project: LEGACY })
  })

  // Case 4: unknown repo falls through the pick tier.
  it('a repoId absent from the pick map falls through to binding/legacy', () => {
    const withOtherRepoPick = settings({
      activeProject: LEGACY,
      activeProjectByRepo: { 'repo-B': PICK }
    })
    expect(
      resolveBoardProject({
        repoId: 'repo-A',
        settings: withOtherRepoPick,
        bindingState: loadedBinding()
      })
    ).toEqual({ source: 'binding', project: BOUND })
    expect(
      resolveBoardProject({
        repoId: 'repo-A',
        settings: withOtherRepoPick,
        bindingState: LOADED_NULL
      })
    ).toEqual({ source: 'legacy', project: LEGACY })
  })

  // Case 5: no result at all.
  it('resolves none when nothing is set', () => {
    const res = resolveBoardProject({
      repoId: 'repo-A',
      settings: settings(),
      bindingState: LOADED_NULL
    })
    expect(res).toEqual({ source: 'none', project: null })
  })

  // Case 6: pending holds, never legacy-flashes.
  it('holds pending while the binding loads, even with legacy set', () => {
    const res = resolveBoardProject({
      repoId: 'repo-A',
      settings: settings({ activeProject: LEGACY }),
      bindingState: LOADING
    })
    expect(res).toEqual({ source: 'pending', project: null })
  })

  // Case 7: pick short-circuits loading.
  it('a pick renders synchronously while the binding loads', () => {
    const res = resolveBoardProject({
      repoId: 'repo-A',
      settings: settings({ activeProjectByRepo: { 'repo-A': PICK } }),
      bindingState: LOADING
    })
    expect(res).toEqual({ source: 'pick', project: PICK, divergesFromBinding: null })
  })

  // Case 8: divergence hint derivation.
  it('derives divergesFromBinding only when the pick differs from the loaded binding', () => {
    const diverging = resolveBoardProject({
      repoId: 'repo-A',
      settings: settings({ activeProjectByRepo: { 'repo-A': PICK } }),
      bindingState: loadedBinding()
    })
    expect(diverging).toEqual({ source: 'pick', project: PICK, divergesFromBinding: BOUND })

    const matching = resolveBoardProject({
      repoId: 'repo-A',
      settings: settings({ activeProjectByRepo: { 'repo-A': { ...BOUND } } }),
      bindingState: loadedBinding()
    })
    expect(matching).toEqual({ source: 'pick', project: BOUND, divergesFromBinding: null })
  })

  // Case 9: partial binding ignored.
  it('ignores an incomplete binding identity and falls to legacy', () => {
    for (const partial of [{ projectOwner: null }, { projectNumber: null }]) {
      const res = resolveBoardProject({
        repoId: 'repo-A',
        settings: settings({ activeProject: LEGACY }),
        bindingState: loadedBinding(partial)
      })
      expect(res).toEqual({ source: 'legacy', project: LEGACY })
    }
  })

  // Case 10: ownerType normalization mirrors resolvePickerProject.
  it('normalizes ownerType: exact organization match, else user', () => {
    const org = resolveBoardProject({
      repoId: 'repo-A',
      settings: settings(),
      bindingState: loadedBinding({ projectOwnerType: 'organization' })
    })
    expect(org.project?.ownerType).toBe('organization')
    for (const raw of ['USER', 'garbage', null]) {
      const res = resolveBoardProject({
        repoId: 'repo-A',
        settings: settings(),
        bindingState: loadedBinding({ projectOwnerType: raw })
      })
      expect(res.project?.ownerType).toBe('user')
    }
  })

  // Case 11: repoId null skips the pick map (standalone surface).
  it('a null repoId never reads the pick map or the binding', () => {
    const res = resolveBoardProject({
      repoId: null,
      settings: settings({ activeProject: LEGACY, activeProjectByRepo: { 'repo-A': PICK } }),
      bindingState: loadedBinding()
    })
    expect(res).toEqual({ source: 'legacy', project: LEGACY })
  })
})

describe('applyBoardPick / clearBoardPick', () => {
  // Case 12: no write-path returns the legacy slot; siblings preserved.
  it('writes the per-repo slot and leaves legacy + other repos byte-untouched', () => {
    const prev = settings({
      activeProject: LEGACY,
      activeProjectByRepo: { 'repo-B': BOUND },
      pinned: [{ owner: 'p', ownerType: 'user', number: 1 }],
      recent: [{ owner: 'old', ownerType: 'user', number: 2, lastOpenedAt: '2026-01-01' }],
      lastViewByProject: { 'user:old:2': { viewId: 'V_old' } }
    })
    const next = applyBoardPick(prev, 'repo-A', { ...PICK, viewId: 'V_new' })
    expect(next.activeProject).toBe(prev.activeProject)
    expect(next.activeProjectByRepo?.['repo-B']).toBe(prev.activeProjectByRepo?.['repo-B'])
    expect(next.activeProjectByRepo?.['repo-A']).toEqual(PICK)
    expect(next.pinned).toBe(prev.pinned)
    // recent + lastViewByProject stay GLOBAL (project-keyed, repo-agnostic).
    expect(next.recent[0]).toMatchObject({
      owner: PICK.owner,
      ownerType: PICK.ownerType,
      number: PICK.number
    })
    expect(next.recent).toHaveLength(2)
    expect(next.lastViewByProject).toEqual({
      'user:old:2': { viewId: 'V_old' },
      'organization:acme:7': { viewId: 'V_new' }
    })
  })

  it('repoId null reproduces the pre-016 standalone shape (writes activeProject)', () => {
    const prev = settings({ activeProject: LEGACY, activeProjectByRepo: { 'repo-B': BOUND } })
    const next = applyBoardPick(prev, null, PICK)
    expect(next.activeProject).toEqual(PICK)
    expect(next.activeProjectByRepo).toBe(prev.activeProjectByRepo)
    expect(next.recent[0]).toMatchObject({ owner: PICK.owner, number: PICK.number })
    // No viewId supplied → no lastViewByProject entry (matches commitSelection).
    expect(next.lastViewByProject).toEqual({})
  })

  it('clearBoardPick deletes exactly one key', () => {
    const prev = settings({
      activeProject: LEGACY,
      activeProjectByRepo: { 'repo-A': PICK, 'repo-B': BOUND }
    })
    const next = clearBoardPick(prev, 'repo-A')
    expect(next.activeProjectByRepo).toEqual({ 'repo-B': BOUND })
    expect(next.activeProject).toBe(prev.activeProject)
    expect(next.recent).toBe(prev.recent)
  })

  // Case 13: missing-map tolerance (upgraded profile without the key).
  it('tolerates a settings object without activeProjectByRepo', () => {
    const prev = settings()
    delete prev.activeProjectByRepo
    const next = applyBoardPick(prev, 'repo-A', PICK)
    expect(next.activeProjectByRepo).toEqual({ 'repo-A': PICK })
    const cleared = clearBoardPick(prev, 'repo-A')
    expect(cleared.activeProjectByRepo).toEqual({})
  })
})
