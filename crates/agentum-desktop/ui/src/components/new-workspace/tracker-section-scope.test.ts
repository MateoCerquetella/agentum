import { describe, expect, it } from 'vitest'
import {
  deriveTrackerIssueViewModel,
  pickerScopeKey,
  resolvePickerProject
} from './work-item-picker-model'
import { deriveUnifiedTrackerStatus } from './create-workspace-wizard-model'
import {
  isCurrentTrackerSectionScope,
  trackerConfigureActionLabel,
  trackerSectionAfterSuccessfulUnbind,
  trackerSectionTableForScope
} from './tracker-section-scope'
import type { GitHubProjectTable } from '@/shared/github-project-types'

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((next) => {
    resolve = next
  })
  return { promise, resolve }
}

function table(repository: string, number: number, title: string): GitHubProjectTable {
  return {
    project: {
      id: 'same-project',
      owner: 'acme',
      ownerType: 'organization',
      number: 7,
      title: 'Shared board',
      url: 'https://github.com/orgs/acme/projects/7'
    },
    selectedView: {
      id: 'view',
      number: 1,
      name: 'Board',
      layout: 'TABLE_LAYOUT',
      filter: '',
      fields: [],
      groupByFields: [],
      sortByFields: []
    },
    rows: [
      {
        id: `${repository}-${number}`,
        itemType: 'ISSUE',
        content: {
          number,
          title,
          body: null,
          url: `https://github.com/${repository}/issues/${number}`,
          state: 'OPEN',
          stateReason: null,
          isDraft: null,
          repository,
          assignees: [],
          labels: [],
          parentIssue: null,
          issueType: null
        },
        fieldValuesByFieldId: {},
        updatedAt: '2026-07-21T00:00:00Z',
        position: 0
      }
    ],
    totalCount: 1,
    parentFieldDropped: false
  }
}

describe('TrackerSection scope lifecycle', () => {
  it('rejects deferred A after switching to B on the same Project', async () => {
    const project = { owner: 'acme', ownerType: 'organization' as const, number: 7 }
    const scopeA = pickerScopeKey({
      targetKey: 'repo-a:/repos/a',
      repositorySlug: 'acme/a',
      project
    })
    const scopeB = pickerScopeKey({
      targetKey: 'repo-b:/repos/b',
      repositorySlug: 'acme/b',
      project
    })
    const requestA = deferred<GitHubProjectTable>()
    const requestB = deferred<GitHubProjectTable>()
    let currentScope: string | null = scopeA
    let rendered = { status: 'loading', count: 0, rows: [] as string[] }

    const commit = async (
      capturedScope: string,
      repositorySlug: string,
      request: Promise<GitHubProjectTable>
    ) => {
      const result = await request
      if (!isCurrentTrackerSectionScope(capturedScope, currentScope)) return
      const view = deriveTrackerIssueViewModel(result, '', repositorySlug)
      rendered = {
        status: 'idle',
        count: view.issueCount,
        rows: view.options.map((option) => option.title)
      }
    }

    const pendingA = commit(scopeA, 'acme/a', requestA.promise)
    currentScope = scopeB
    rendered = { status: 'loading', count: 0, rows: [] }
    const pendingB = commit(scopeB, 'acme/b', requestB.promise)
    requestB.resolve(table('acme/b', 2, 'B issue'))
    await pendingB
    expect(rendered).toEqual({ status: 'idle', count: 1, rows: ['B issue'] })

    requestA.resolve(table('acme/a', 1, 'A issue'))
    await pendingA
    expect(rendered).toEqual({ status: 'idle', count: 1, rows: ['B issue'] })
  })

  it('projects successful unbind to Configure with no connected data and rejects old completions', () => {
    const project = { owner: 'acme', ownerType: 'organization' as const, number: 7 }
    const targetKey = 'repo-a:/repos/a'
    const oldScope = pickerScopeKey({
      targetKey,
      repositorySlug: 'acme/a',
      project
    })
    const oldTable = table('acme/a', 1, 'Old issue')
    const unbound = trackerSectionAfterSuccessfulUnbind(targetKey)
    const resolved = resolvePickerProject({
      binding: unbound.binding,
      activeProject: { owner: 'global', ownerType: 'organization', number: 99 },
      selectedGitRepo: true
    })
    const eligibleTable = trackerSectionTableForScope(
      { scopeKey: oldScope, table: oldTable },
      unbound.scopeKey,
      null
    )
    const view = deriveTrackerIssueViewModel(eligibleTable, '', 'acme/a')
    const status = deriveUnifiedTrackerStatus({
      resolved,
      binding: unbound.binding,
      selectedGitRepo: true,
      status: 'idle',
      optionCount: view.issueCount,
      hasTable: Boolean(eligibleTable)
    })

    expect(trackerConfigureActionLabel(Boolean(resolved))).toBe('Configure tracker')
    expect(status).toEqual({ kind: 'none' })
    expect(view.issueCount).toBe(0)
    expect(view.options).toEqual([])
    expect(isCurrentTrackerSectionScope(oldScope, unbound.scopeKey)).toBe(false)
  })
})
