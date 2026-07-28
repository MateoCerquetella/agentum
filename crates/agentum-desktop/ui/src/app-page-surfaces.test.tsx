// @vitest-environment happy-dom
import React, { Suspense } from 'react'
import { act, create, type ReactTestRenderer } from 'react-test-renderer'
import { describe, expect, it, vi } from 'vitest'
import { AppPageSurface } from './App'

;(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true

function surface(name: string): () => React.JSX.Element {
  return () => <div data-page-surface={name}>{name}</div>
}

vi.mock('./components/settings/Settings', () => ({ default: surface('settings') }))
vi.mock('./components/TaskPage', () => ({ default: surface('tasks') }))
vi.mock('./components/mission-control/MissionControlPage', () => ({
  default: surface('mission-control')
}))
vi.mock('./components/project-hub/ProjectHubPage', () => ({ default: surface('project') }))
vi.mock('./components/projects/ProjectsPage', () => ({ default: surface('projects') }))

describe('App top-level page surfaces', () => {
  it.each([
    ['settings', null, 'settings'],
    ['tasks', null, 'tasks'],
    ['activity', null, 'mission-control'],
    ['project', null, 'project'],
    ['projects', null, 'projects'],
    ['terminal', null, 'mission-control']
  ] as const)('mounts %s as %s', async (activeView, activeWorktreeId, expected) => {
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <Suspense fallback={<div>loading</div>}>
          <AppPageSurface activeView={activeView} activeWorktreeId={activeWorktreeId} />
        </Suspense>
      )
    })
    await act(async () => {
      await vi.waitFor(
        () => {
          expect(renderer!.root.findByProps({ 'data-page-surface': expected })).toBeTruthy()
        },
        { timeout: 15_000 }
      )
    })

    expect(renderer!.root.findByProps({ 'data-page-surface': expected })).toBeTruthy()
    await act(async () => renderer!.unmount())
  }, 20_000)

  it('keeps the terminal workbench route free of a competing page surface', async () => {
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <Suspense fallback={<div>loading</div>}>
          <AppPageSurface activeView="terminal" activeWorktreeId="worktree-1" />
        </Suspense>
      )
    })

    expect(renderer!.toJSON()).toBeNull()
    await act(async () => renderer!.unmount())
  })
})
