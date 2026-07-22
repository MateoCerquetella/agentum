import { describe, expect, it } from 'vitest'
import type { Repo } from '@/shared/types'
import { githubProjectTaskScope, linearProjectTaskScope, projectTaskScopeKey, unboundProjectTaskScope } from './project-task-scope'

const repo = { id: 'repo-a', displayName: 'Repo A', path: '/a', badgeColor: '#000', addedAt: 1, trackerProvider: 'linear', linearProjectBinding: { workspaceId: 'w-a', workspaceName: 'W', projectId: 'p-a', projectName: 'P' } } as Repo

describe('project task scope', () => {
  it('uses non-lossy provider identities', () => expect(projectTaskScopeKey({ repoId: 'r', provider: 'linear', workspaceId: 'w', projectId: 'p' })).toBe('["r","linear","w","p"]'))
  it('does not bind auto or absent providers', () => expect(unboundProjectTaskScope({ ...repo, trackerProvider: 'auto' }, 1)).toMatchObject({ status: 'unbound', reason: 'provider-unset' }))
  it('rejects a Linear response from another project', () => expect(linearProjectTaskScope(repo, 2, { id: 'p-b', workspaceId: 'w-a', name: 'Other' })).toMatchObject({ status: 'unavailable', reason: 'invalid-binding' }))
  it('preserves the full GitHub binding identity', () => expect(githubProjectTaskScope({ ...repo, trackerProvider: 'github' }, 3, 'o/r', { projectId: 'PVT', statusFieldId: 'f', statusMapping: { todo: 'a', inProgress: 'b', readyToTest: 'c', done: 'd', blocked: 'e' }, doneClosesIssue: true, projectTitle: 'Board', projectOwner: 'o', projectOwnerType: 'organization', projectNumber: 7, optionNames: null })).toMatchObject({ status: 'bound', repoSlug: 'o/r', projectId: 'PVT', owner: 'o', projectNumber: 7 }))
})
