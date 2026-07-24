import { beforeEach, describe, expect, it, vi } from 'vitest'
import { create } from 'zustand'
import type { AppState } from '../types'
import type { ProjectTrackerConfig } from '@/shared/project-tracker-config'
import { createProjectTrackersSlice } from './project-trackers'
import { ProjectTrackerConflictError } from '@/runtime/server-project-tracker-client'

const client = vi.hoisted(() => ({
  getProjectTrackerConfig: vi.fn(),
  putProjectTrackerConfig: vi.fn()
}))

vi.mock('@/runtime/server-project-tracker-client', () => {
  class MockProjectTrackerConflictError extends Error {
    readonly current: ProjectTrackerConfig | null

    constructor(current: ProjectTrackerConfig | null) {
      super('Tracker settings changed elsewhere. Review the latest settings and try again.')
      this.name = 'ProjectTrackerConflictError'
      this.current = current
    }
  }

  return {
    ...client,
    ProjectTrackerConflictError: MockProjectTrackerConflictError
  }
})

function githubConfig(repoId: string, revision: number): ProjectTrackerConfig {
  return {
    schemaVersion: 1,
    repoId,
    revision,
    provider: 'github',
    github: { repositorySlug: `acme/${repoId}` },
    taskPreferences: {},
    provenance: 'configured'
  }
}

function makeStore() {
  return create<AppState>()(
    (...args) => ({
      ...createProjectTrackersSlice(...args)
    }) as AppState
  )
}

describe('project tracker config store', () => {
  beforeEach(() => {
    client.getProjectTrackerConfig.mockReset()
    client.putProjectTrackerConfig.mockReset()
  })

  it('isolates repo buckets and deduplicates an in-flight read per repo', async () => {
    let resolveA: ((value: { config: ProjectTrackerConfig }) => void) | undefined
    client.getProjectTrackerConfig.mockImplementation((repoId: string) => {
      if (repoId === 'repo-a') {
        return new Promise((resolve) => {
          resolveA = resolve
        })
      }
      return Promise.resolve({ config: githubConfig(repoId, 2) })
    })
    const store = makeStore()

    const firstA = store.getState().loadProjectTrackerConfig('repo-a')
    const secondA = store.getState().loadProjectTrackerConfig('repo-a')
    const loadB = store.getState().loadProjectTrackerConfig('repo-b')

    expect(client.getProjectTrackerConfig).toHaveBeenCalledTimes(2)
    await expect(loadB).resolves.toEqual(githubConfig('repo-b', 2))
    expect(store.getState().projectTrackerConfigByRepo['repo-b']).toEqual(
      githubConfig('repo-b', 2)
    )
    expect(store.getState().projectTrackerConfigByRepo['repo-a']).toBeUndefined()

    resolveA?.({ config: githubConfig('repo-a', 1) })
    await expect(Promise.all([firstA, secondA])).resolves.toEqual([
      githubConfig('repo-a', 1),
      githubConfig('repo-a', 1)
    ])
    expect(store.getState().projectTrackerLoadStatusByRepo['repo-a']).toBe('loaded')
  })

  it('writes with the cached revision and installs the saved record', async () => {
    const store = makeStore()
    const current = githubConfig('repo-c', 7)
    const draft = { ...current, taskPreferences: { github: { query: 'label:ready' } } }
    const saved = { ...draft, revision: 8 }
    store.setState({
      projectTrackerConfigByRepo: { 'repo-c': current },
      projectTrackerLoadStatusByRepo: { 'repo-c': 'loaded' }
    })
    client.putProjectTrackerConfig.mockResolvedValueOnce(saved)

    await expect(store.getState().saveProjectTrackerConfig('repo-c', draft)).resolves.toEqual(saved)

    expect(client.putProjectTrackerConfig).toHaveBeenCalledWith('repo-c', draft, 7)
    expect(store.getState().projectTrackerConfigByRepo['repo-c']).toEqual(saved)
    expect(store.getState().projectTrackerSavingByRepo['repo-c']).toBe(false)
  })

  it('reconciles a compare-and-swap conflict to the authoritative record', async () => {
    const store = makeStore()
    const current = githubConfig('repo-d', 3)
    const authoritative = githubConfig('repo-d', 9)
    store.setState({
      projectTrackerConfigByRepo: { 'repo-d': current },
      projectTrackerLoadStatusByRepo: { 'repo-d': 'loaded' }
    })
    client.putProjectTrackerConfig.mockRejectedValueOnce(
      new ProjectTrackerConflictError(authoritative)
    )

    await expect(store.getState().saveProjectTrackerConfig('repo-d', current)).rejects.toBeInstanceOf(
      ProjectTrackerConflictError
    )

    expect(store.getState().projectTrackerConfigByRepo['repo-d']).toEqual(authoritative)
    expect(store.getState().projectTrackerLoadStatusByRepo['repo-d']).toBe('loaded')
    expect(store.getState().projectTrackerErrorByRepo['repo-d']).toContain(
      'changed elsewhere'
    )
    expect(store.getState().projectTrackerSavingByRepo['repo-d']).toBe(false)
  })

  it('forgets every repo bucket and ignores a stale in-flight response', async () => {
    let resolve: ((value: { config: ProjectTrackerConfig; migrationConflict?: string }) => void) | undefined
    client.getProjectTrackerConfig.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    const store = makeStore()
    const pending = store.getState().loadProjectTrackerConfig('repo-e')
    store.setState({
      projectTrackerMigrationConflictByRepo: { 'repo-e': 'legacy conflict' },
      projectTrackerSavingByRepo: { 'repo-e': true }
    })

    store.getState().forgetProjectTrackerConfig('repo-e')
    resolve?.({ config: githubConfig('repo-e', 1), migrationConflict: 'stale' })
    await pending

    expect(store.getState().projectTrackerConfigByRepo['repo-e']).toBeUndefined()
    expect(store.getState().projectTrackerLoadStatusByRepo['repo-e']).toBeUndefined()
    expect(store.getState().projectTrackerErrorByRepo['repo-e']).toBeUndefined()
    expect(store.getState().projectTrackerMigrationConflictByRepo['repo-e']).toBeUndefined()
    expect(store.getState().projectTrackerSavingByRepo['repo-e']).toBeUndefined()
  })
})
