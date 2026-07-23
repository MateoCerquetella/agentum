import type { StateCreator } from 'zustand'
import type { AppState } from '../types'
import type { ProjectTrackerConfig } from '@/shared/project-tracker-config'
import {
  getProjectTrackerConfig,
  ProjectTrackerConflictError,
  putProjectTrackerConfig
} from '@/runtime/server-project-tracker-client'

export type ProjectTrackerLoadStatus = 'idle' | 'loading' | 'loaded' | 'error'

const inflightLoads = new Map<string, Promise<ProjectTrackerConfig | null>>()
const loadGenerations = new Map<string, number>()

function nextGeneration(repoId: string): number {
  const generation = (loadGenerations.get(repoId) ?? 0) + 1
  loadGenerations.set(repoId, generation)
  return generation
}

function currentGeneration(repoId: string, generation: number): boolean {
  return loadGenerations.get(repoId) === generation
}

export type ProjectTrackersSlice = {
  /** Missing key = never loaded; null = authoritatively loaded without a row. */
  projectTrackerConfigByRepo: Record<string, ProjectTrackerConfig | null>
  projectTrackerLoadStatusByRepo: Record<string, ProjectTrackerLoadStatus>
  projectTrackerErrorByRepo: Record<string, string | undefined>
  projectTrackerMigrationConflictByRepo: Record<string, string | undefined>
  projectTrackerSavingByRepo: Record<string, boolean>
  loadProjectTrackerConfig: (
    repoId: string,
    options?: { force?: boolean }
  ) => Promise<ProjectTrackerConfig | null>
  saveProjectTrackerConfig: (
    repoId: string,
    config: ProjectTrackerConfig
  ) => Promise<ProjectTrackerConfig>
  forgetProjectTrackerConfig: (repoId: string) => void
}

export const createProjectTrackersSlice: StateCreator<
  AppState,
  [],
  [],
  ProjectTrackersSlice
> = (set, get) => ({
  projectTrackerConfigByRepo: {},
  projectTrackerLoadStatusByRepo: {},
  projectTrackerErrorByRepo: {},
  projectTrackerMigrationConflictByRepo: {},
  projectTrackerSavingByRepo: {},

  loadProjectTrackerConfig: async (repoId, options) => {
    const force = options?.force === true
    const current = get().projectTrackerConfigByRepo[repoId]
    if (!force && current !== undefined) return current
    const inflight = inflightLoads.get(repoId)
    if (!force && inflight) return inflight

    const generation = nextGeneration(repoId)
    set((state) => ({
      projectTrackerLoadStatusByRepo: {
        ...state.projectTrackerLoadStatusByRepo,
        [repoId]: 'loading'
      },
      projectTrackerErrorByRepo: { ...state.projectTrackerErrorByRepo, [repoId]: undefined }
    }))
    const promise = getProjectTrackerConfig(repoId)
      .then((response) => {
        if (currentGeneration(repoId, generation)) {
          set((state) => ({
            projectTrackerConfigByRepo: {
              ...state.projectTrackerConfigByRepo,
              [repoId]: response.config
            },
            projectTrackerLoadStatusByRepo: {
              ...state.projectTrackerLoadStatusByRepo,
              [repoId]: 'loaded'
            },
            projectTrackerErrorByRepo: {
              ...state.projectTrackerErrorByRepo,
              [repoId]: undefined
            },
            projectTrackerMigrationConflictByRepo: {
              ...state.projectTrackerMigrationConflictByRepo,
              [repoId]: response.migrationConflict
            }
          }))
        }
        return response.config
      })
      .catch((cause: unknown) => {
        if (currentGeneration(repoId, generation)) {
          const message = cause instanceof Error ? cause.message : String(cause)
          set((state) => ({
            projectTrackerLoadStatusByRepo: {
              ...state.projectTrackerLoadStatusByRepo,
              [repoId]: 'error'
            },
            projectTrackerErrorByRepo: {
              ...state.projectTrackerErrorByRepo,
              [repoId]: message
            }
          }))
        }
        throw cause
      })
      .finally(() => {
        if (inflightLoads.get(repoId) === promise) inflightLoads.delete(repoId)
      })
    inflightLoads.set(repoId, promise)
    return promise
  },

  saveProjectTrackerConfig: async (repoId, config) => {
    if (config.repoId !== repoId) {
      throw new Error('Tracker config does not belong to the selected project.')
    }
    if (get().projectTrackerConfigByRepo[repoId] === undefined) {
      await get().loadProjectTrackerConfig(repoId)
    }
    const current = get().projectTrackerConfigByRepo[repoId] ?? null
    set((state) => ({
      projectTrackerSavingByRepo: { ...state.projectTrackerSavingByRepo, [repoId]: true },
      projectTrackerErrorByRepo: { ...state.projectTrackerErrorByRepo, [repoId]: undefined }
    }))
    try {
      const saved = await putProjectTrackerConfig(repoId, config, current?.revision ?? null)
      nextGeneration(repoId)
      set((state) => ({
        projectTrackerConfigByRepo: { ...state.projectTrackerConfigByRepo, [repoId]: saved },
        projectTrackerLoadStatusByRepo: {
          ...state.projectTrackerLoadStatusByRepo,
          [repoId]: 'loaded'
        },
        projectTrackerErrorByRepo: { ...state.projectTrackerErrorByRepo, [repoId]: undefined },
        projectTrackerMigrationConflictByRepo: {
          ...state.projectTrackerMigrationConflictByRepo,
          [repoId]: undefined
        }
      }))
      return saved
    } catch (cause) {
      if (cause instanceof ProjectTrackerConflictError) {
        nextGeneration(repoId)
        set((state) => ({
          projectTrackerConfigByRepo: {
            ...state.projectTrackerConfigByRepo,
            [repoId]: cause.current
          },
          projectTrackerLoadStatusByRepo: {
            ...state.projectTrackerLoadStatusByRepo,
            [repoId]: 'loaded'
          },
          projectTrackerErrorByRepo: {
            ...state.projectTrackerErrorByRepo,
            [repoId]: cause.message
          }
        }))
      } else {
        const message = cause instanceof Error ? cause.message : String(cause)
        set((state) => ({
          projectTrackerErrorByRepo: {
            ...state.projectTrackerErrorByRepo,
            [repoId]: message
          }
        }))
      }
      throw cause
    } finally {
      set((state) => ({
        projectTrackerSavingByRepo: { ...state.projectTrackerSavingByRepo, [repoId]: false }
      }))
    }
  },

  forgetProjectTrackerConfig: (repoId) => {
    nextGeneration(repoId)
    inflightLoads.delete(repoId)
    set((state) => {
      const projectTrackerConfigByRepo = { ...state.projectTrackerConfigByRepo }
      const projectTrackerLoadStatusByRepo = { ...state.projectTrackerLoadStatusByRepo }
      const projectTrackerErrorByRepo = { ...state.projectTrackerErrorByRepo }
      const projectTrackerMigrationConflictByRepo = {
        ...state.projectTrackerMigrationConflictByRepo
      }
      const projectTrackerSavingByRepo = { ...state.projectTrackerSavingByRepo }
      delete projectTrackerConfigByRepo[repoId]
      delete projectTrackerLoadStatusByRepo[repoId]
      delete projectTrackerErrorByRepo[repoId]
      delete projectTrackerMigrationConflictByRepo[repoId]
      delete projectTrackerSavingByRepo[repoId]
      return {
        projectTrackerConfigByRepo,
        projectTrackerLoadStatusByRepo,
        projectTrackerErrorByRepo,
        projectTrackerMigrationConflictByRepo,
        projectTrackerSavingByRepo
      }
    })
  }
})
