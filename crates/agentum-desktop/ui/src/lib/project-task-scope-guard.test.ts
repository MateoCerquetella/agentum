import { describe, expect, it } from 'vitest'
import type { ProjectTaskScope } from './project-task-scope'
import type { ProjectTrackerConfig } from '@/shared/project-tracker-config'
import { captureProjectTaskScopeGuard, isProjectTaskScopeGuardCurrent, linearActionMatchesScope, linearIssueMatchesScope, projectTaskScopeGuardMatchesTracker } from './project-task-scope-guard'

const scope = { status: 'bound', provider: 'linear', repoId: 'r', repoName: 'R', generation: 2, scopeKey: 'key', workspaceId: 'w', workspaceName: 'W', projectId: 'p', projectName: 'P', teamIds: ['t'] } as const satisfies ProjectTaskScope

describe('project scope guard', () => {
  it('rejects late responses after a generation change', () => { const guard = captureProjectTaskScopeGuard(scope)!; expect(isProjectTaskScopeGuardCurrent(guard, { ...scope, generation: 3 })).toBe(false) })
  it('rejects mismatched Linear read and write identities', () => {
    expect(linearIssueMatchesScope({ id: 'i', workspaceId: 'other', identifier: 'X-1', title: 'x', url: 'https://linear.app/i', state: { name: 'Todo', type: 'unstarted', color: '' }, team: { id: 't', name: 'T', key: 'T' }, project: { id: 'p', workspaceId: 'other', name: 'P' }, labels: [], labelIds: [], priority: 0, updatedAt: '' }, scope)).toBe(false)
    expect(linearActionMatchesScope({ workspaceId: 'w', projectId: 'other', teamId: 't' }, scope)).toBe(false)
  })

  it('accepts a canonical GitHub repository scope without requiring a Project binding', () => {
    const tracker: ProjectTrackerConfig = {
      schemaVersion: 1,
      repoId: 'r',
      revision: 1,
      provider: 'github',
      github: { repositorySlug: 'acme/widgets' },
      taskPreferences: {},
      provenance: 'configured'
    }
    expect(
      projectTaskScopeGuardMatchesTracker(
        {
          repoId: 'r',
          generation: 2,
          scopeKey: '["r","github","acme/widgets","repository"]'
        },
        tracker
      )
    ).toBe(true)
    expect(
      projectTaskScopeGuardMatchesTracker(
        {
          repoId: 'r',
          generation: 2,
          scopeKey: '["r","github","acme/widgets","project-id"]'
        },
        tracker
      )
    ).toBe(false)
  })
})
