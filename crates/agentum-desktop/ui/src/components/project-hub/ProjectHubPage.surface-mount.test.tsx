// @vitest-environment happy-dom
import React from 'react'
import { act, create, type ReactTestInstance, type ReactTestRenderer } from 'react-test-renderer'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repo } from '@/shared/types'
import { useAppStore } from '@/store'

;(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true

vi.mock('@/components/sdd/SddWorkspaceBar', () => ({
  default: () => <div data-hub-surface="specs">specs surface</div>
}))
vi.mock('@/components/wiki/WikiPage', () => ({
  default: () => <div data-hub-surface="wiki">wiki surface</div>
}))
vi.mock('./ProjectTasksPage', () => ({
  ProjectTasksPage: () => <div data-hub-surface="tasks">tasks surface</div>
}))
vi.mock('./ProjectSessionsList', () => ({
  ProjectSessionsList: () => <div data-hub-surface="sessions">sessions surface</div>
}))

function textOf(node: ReactTestInstance | string | number): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  return node.children.map((child) => textOf(child as ReactTestInstance | string | number)).join('')
}

function tab(root: ReactTestInstance, label: string): ReactTestInstance {
  const match = root
    .findAllByType('button')
    .find((entry) => textOf(entry).trim().startsWith(label))
  if (!match) throw new Error(`Project tab not found: ${label}`)
  return match
}

describe('Project Hub production surfaces', () => {
  beforeEach(() => {
    const repo: Repo = {
      id: 'repo-1',
      path: '/workspace/demo',
      displayName: 'Demo Project',
      badgeColor: '#336699',
      addedAt: 1
    }
    useAppStore.setState({
      repos: [repo],
      activeRepoId: repo.id,
      projectHubTab: 'specs',
      worktreesByRepo: { [repo.id]: [] }
    })
  })

  afterEach(() => {
    useAppStore.setState({
      repos: [],
      activeRepoId: null,
      projectHubTab: 'specs',
      worktreesByRepo: {}
    })
  })

  it('mounts Specs, Wiki, Tasks, Sessions, and the tracker compatibility route', async () => {
    const { default: ProjectHubPage } = await import('./ProjectHubPage')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(<ProjectHubPage />)
      await Promise.resolve()
    })

    expect(renderer!.root.findByProps({ 'data-hub-surface': 'specs' })).toBeTruthy()
    expect(['Specs', 'Wiki', 'Tasks', 'Sessions'].map((label) => textOf(tab(renderer!.root, label)))).toEqual([
      'Specs',
      'Wiki',
      'Tasks',
      'Sessions0'
    ])

    for (const [label, surface] of [
      ['Wiki', 'wiki'],
      ['Tasks', 'tasks'],
      ['Sessions', 'sessions'],
      ['Specs', 'specs']
    ] as const) {
      await act(async () => {
        tab(renderer!.root, label).props.onClick()
        await Promise.resolve()
      })
      expect(renderer!.root.findByProps({ 'data-hub-surface': surface })).toBeTruthy()
    }

    await act(async () => useAppStore.getState().setProjectHubTab('tracker'))
    expect(renderer!.root.findByProps({ 'data-hub-surface': 'tasks' })).toBeTruthy()
    expect(tab(renderer!.root, 'Tasks').props['aria-current']).toBe('page')
    await act(async () => renderer!.unmount())
  })
})
