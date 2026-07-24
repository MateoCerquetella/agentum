import { describe, expect, it } from 'vitest'
import type { TaskResumeState } from './types'
import {
  resolveLinearContextForRepo,
  taskProjectScopeKey
} from './task-project-scope'

describe('task project scope', () => {
  it('uses a reserved scope when there is no active repo', () => {
    expect(taskProjectScopeKey(null)).toBe('global')
    expect(taskProjectScopeKey('repo-x')).toBe('repo-x')
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
