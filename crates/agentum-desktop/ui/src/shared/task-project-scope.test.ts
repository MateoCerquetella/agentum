import { describe, expect, it } from 'vitest'
import type { GitHubProjectSettings } from './github-project-types'
import type { TaskResumeState } from './types'
import {
  resolveActiveProjectForRepo,
  resolveLinearContextForRepo,
  taskProjectScopeKey
} from './task-project-scope'

describe('task project scope', () => {
  it('uses a reserved scope when there is no active repo', () => {
    expect(taskProjectScopeKey(null)).toBe('global')
    expect(taskProjectScopeKey('repo-x')).toBe('repo-x')
  })

  it('resolves GitHub projects only from the current repo binding', () => {
    const project = { owner: 'acme', ownerType: 'organization' as const, number: 7 }
    const settings: GitHubProjectSettings = {
      pinned: [],
      recent: [],
      lastViewByProject: {},
      activeProject: { owner: 'legacy', ownerType: 'user', number: 1 },
      activeProjectByRepo: { 'repo-x': project }
    }

    expect(resolveActiveProjectForRepo(settings, 'repo-x')).toEqual(project)
    expect(resolveActiveProjectForRepo(settings, 'repo-y')).toBeNull()
  })

  it('resolves Linear contexts only from the current repo binding', () => {
    const context = { kind: 'project' as const, id: 'project-x', workspaceId: 'workspace-1' }
    const resume: TaskResumeState = {
      linearContext: { kind: 'project', id: 'legacy', workspaceId: 'workspace-1' },
      linearContextByRepo: { 'repo-x': context }
    }

    expect(resolveLinearContextForRepo(resume, 'repo-x')).toEqual(context)
    expect(resolveLinearContextForRepo(resume, 'repo-y')).toBeUndefined()
  })
})
