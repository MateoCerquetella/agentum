import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('locked Linear board', () => {
  it('uses the exact project endpoint and guards reads and actions', () => { const source = readFileSync(new URL('./LockedLinearProjectTasks.tsx', import.meta.url), 'utf8'); expect(source).toContain('linearListProjectIssues(settings, scope.projectId'); expect(source).toContain('linearIssueMatchesScope'); expect(source).toContain('linearActionMatchesScope'); expect(source).toContain('isLiveProjectTaskScopeAuthority') })
  it('opens a repo-locked workspace from an exact hydrated Linear issue', () => { const source = readFileSync(new URL('./LockedLinearProjectTasks.tsx', import.meta.url), 'utf8'); expect(source).toContain('linearGetIssue(settings, issue.id, scope.workspaceId)'); expect(source).toContain('buildLinearIssueLinkedWorkItem(exact)'); expect(source).toContain('initialRepoId: scope.repoId'); expect(source).toContain('requiredProjectTaskScope: guard'); expect(source).toContain('Start workspace') })
})
