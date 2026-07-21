import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { GitHubProjectTable } from '../../../../shared/github-project-types'
import type { AppState } from '@/store/types'
import { projectViewCacheKey } from '@/store/slices/github'

const mockStore = vi.hoisted(() => {
  const current = {} as AppState
  const useAppStore = Object.assign(
    <T,>(selector: (state: AppState) => T): T => selector(current),
    {
      getState: (): AppState => current,
      setState: (
        update: Partial<AppState> | ((state: AppState) => Partial<AppState>)
      ): void => {
        Object.assign(current, typeof update === 'function' ? update(current) : update)
      }
    }
  )
  return { current, useAppStore }
})

vi.mock('@/store', () => ({ useAppStore: mockStore.useAppStore }))

vi.mock('./ProjectPicker', () => ({
  default: ({ activeProject }: { activeProject: { owner: string; number: number } | null }) => (
    <div data-testid="project-picker">
      {activeProject ? `${activeProject.owner}/#${activeProject.number}` : 'no-project'}
    </div>
  )
}))

vi.mock('./ProjectViewList', () => ({
  default: ({ table }: { table: GitHubProjectTable }) => (
    <div data-testid="project-table">{table.project.title}</div>
  )
}))

vi.mock('./ProjectBoardView', () => ({
  default: ({ table }: { table: GitHubProjectTable }) => (
    <div data-testid="project-board">{table.project.title}</div>
  )
}))

vi.mock('@/components/GitHubItemDialog', () => ({ default: () => null }))
vi.mock('./ProjectItemSlugDialog', () => ({ default: () => null }))
vi.mock('./use-project-view-live-refresh', () => ({ useProjectViewLiveRefresh: () => undefined }))
vi.mock('@/lib/repo-slug-index', () => ({
  useRepoSlugIndex: () => ({ lookupSlug: () => [], ready: true })
}))

import ProjectViewWrapper from './ProjectViewWrapper'
import { useAppStore } from '@/store'

const AGENTUM_PROJECT_KEY = 'user:MateoCerquetella:2'
const AGENTUM_VIEW_ID = 'view-agentum'
const AGENTUM_CACHE_KEY = projectViewCacheKey(
  'user',
  'MateoCerquetella',
  2,
  AGENTUM_VIEW_ID
)

const agentumTable = {
  project: {
    id: 'project-agentum',
    title: 'Agentum board',
    url: 'https://github.com/users/MateoCerquetella/projects/2'
  },
  selectedView: {
    id: AGENTUM_VIEW_ID,
    number: 1,
    name: 'Roadmap',
    layout: 'TABLE_LAYOUT',
    filter: ''
  },
  fields: [],
  rows: [],
  totalCount: 0,
  parentFieldDropped: false
} as unknown as GitHubProjectTable

describe('ProjectViewWrapper repo switch isolation', () => {
  beforeEach(() => {
    useAppStore.setState({
      repos: [],
      settings: {
        githubProjects: {
          pinned: [],
          recent: [],
          activeProject: { owner: 'MateoCerquetella', ownerType: 'user', number: 2 },
          activeProjectByRepo: {},
          lastViewByProject: { [AGENTUM_PROJECT_KEY]: { viewId: AGENTUM_VIEW_ID } }
        }
      } as AppState['settings'],
      projectBindingByRepo: {
        agentum: {
          status: 'loaded',
          binding: {
            projectOwner: 'MateoCerquetella',
            projectOwnerType: 'user',
            projectNumber: 2,
            projectTitle: 'Agentum board'
          }
        },
        freebee: { status: 'loaded', binding: null }
      },
      projectViewCache: {
        [AGENTUM_CACHE_KEY]: {
          data: agentumTable,
          fetchedAt: Date.now()
        }
      },
      fetchProjectViewTable: vi.fn(),
      updateProjectFieldValue: vi.fn(),
      clearProjectFieldValue: vi.fn(),
      patchProjectIssueOrPr: vi.fn(),
      patchProjectRowIssueType: vi.fn(),
      addRepo: vi.fn(),
      updateSettings: vi.fn(),
      openModal: vi.fn()
    } as Partial<AppState>)
  })

  it('drops the previous project identity and table while Freebee is verified, then shows its picker', () => {
    const agentum = renderToStaticMarkup(<ProjectViewWrapper repoId="agentum" />)
    expect(agentum).toContain('MateoCerquetella/#2')
    expect(agentum).toContain('Agentum board')

    // This is the atomic state written by openProjectHub("freebee") before
    // React can render the new TaskPage/ProjectViewWrapper tree.
    useAppStore.setState((state) => ({
      projectBindingByRepo: {
        ...state.projectBindingByRepo,
        freebee: { status: 'loading' }
      }
    }))
    const pendingFreebee = renderToStaticMarkup(<ProjectViewWrapper repoId="freebee" />)
    expect(pendingFreebee).not.toContain('MateoCerquetella/#2')
    expect(pendingFreebee).not.toContain('Agentum board')
    expect(pendingFreebee).toContain('animate-pulse')

    useAppStore.setState((state) => ({
      projectBindingByRepo: {
        ...state.projectBindingByRepo,
        freebee: { status: 'loaded', binding: null }
      }
    }))
    const unboundFreebee = renderToStaticMarkup(<ProjectViewWrapper repoId="freebee" />)
    expect(unboundFreebee).toContain('no-project')
    expect(unboundFreebee).toContain('Choose a project to get started.')
    expect(unboundFreebee).not.toContain('MateoCerquetella/#2')
    expect(unboundFreebee).not.toContain('Agentum board')
  })

  it('treats a missing embedded cache entry as pending, never as a legacy project', () => {
    useAppStore.setState((state) => {
      const projectBindingByRepo = { ...state.projectBindingByRepo }
      delete projectBindingByRepo.freebee
      return { projectBindingByRepo }
    })

    const freebee = renderToStaticMarkup(<ProjectViewWrapper repoId="freebee" />)
    expect(freebee).toContain('animate-pulse')
    expect(freebee).not.toContain('MateoCerquetella/#2')
    expect(freebee).not.toContain('Agentum board')
  })
})
