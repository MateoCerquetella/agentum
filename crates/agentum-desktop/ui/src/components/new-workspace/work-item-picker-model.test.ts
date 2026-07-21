import { describe, expect, it } from 'vitest'
import type {
  GitHubProjectRow,
  GitHubProjectTable
} from '../../shared/github-project-types'
import {
  buildBindPayload,
  deriveIssueOptions,
  deriveTrackerIssueViewModel,
  deriveTrackerBindCoords,
  isPickableIssueRow,
  pickerProjectKey,
  pickerScopeKey,
  resolvePickerProject
} from './work-item-picker-model'

// A minimal Project row builder — only the fields the picker reads matter; the
// rest are filled with inert defaults so a fixture stays a one-liner.
function row(overrides: {
  id?: string
  itemType?: GitHubProjectRow['itemType']
  number?: number | null
  title?: string
  url?: string | null
  state?: string | null
  isDraft?: boolean | null
  repository?: string | null
}): GitHubProjectRow {
  return {
    id: overrides.id ?? 'PVTI_x',
    itemType: overrides.itemType ?? 'ISSUE',
    content: {
      number: overrides.number === undefined ? 1 : overrides.number,
      title: overrides.title ?? 'A task',
      body: null,
      url: overrides.url === undefined ? 'https://github.com/o/r/issues/1' : overrides.url,
      state: overrides.state === undefined ? 'OPEN' : overrides.state,
      stateReason: null,
      isDraft: overrides.isDraft ?? null,
      repository: overrides.repository === undefined ? 'o/r' : overrides.repository,
      assignees: [],
      labels: [],
      parentIssue: null,
      issueType: null
    },
    fieldValuesByFieldId: {},
    updatedAt: '2026-07-08T00:00:00Z',
    position: 0
  }
}

function table(rows: GitHubProjectRow[]): GitHubProjectTable {
  return {
    project: {
      id: 'PVT_1',
      owner: 'o',
      ownerType: 'organization',
      number: 1,
      title: 'Roadmap',
      url: 'https://github.com/orgs/o/projects/1'
    },
    selectedView: {
      id: 'V_1',
      number: 1,
      name: 'Board',
      layout: 'TABLE_LAYOUT',
      filter: '',
      fields: [],
      groupByFields: [],
      sortByFields: []
    },
    rows,
    totalCount: rows.length,
    parentFieldDropped: false
  }
}

describe('deriveIssueOptions', () => {
  // The architect-pinned first-failing test.
  it('excludes PRs and closed issues', () => {
    const options = deriveIssueOptions(
      table([
        row({ id: 'i-open', number: 10, url: 'https://github.com/o/r/issues/10', state: 'OPEN' }),
        row({ id: 'i-closed', number: 11, url: 'https://github.com/o/r/issues/11', state: 'CLOSED' }),
        row({
          id: 'pr',
          itemType: 'PULL_REQUEST',
          number: 12,
          url: 'https://github.com/o/r/pull/12',
          state: 'OPEN',
          isDraft: false
        }),
        row({
          id: 'draft',
          itemType: 'DRAFT_ISSUE',
          number: null,
          url: null
        }),
        row({ id: 'redacted', itemType: 'REDACTED', number: null, url: null })
      ])
    )
    expect(options.map((o) => o.itemId)).toEqual(['i-open'])
    expect(options[0].number).toBe(10)
    expect(options[0].url).toBe('https://github.com/o/r/issues/10')
  })

  it('is empty for a null/empty table (the honest empty state, AC 3)', () => {
    expect(deriveIssueOptions(null)).toEqual([])
    expect(deriveIssueOptions(undefined)).toEqual([])
    expect(deriveIssueOptions(table([]))).toEqual([])
  })

  it('keeps fetched order and dedupes by issue URL', () => {
    const options = deriveIssueOptions(
      table([
        row({ id: 'a', number: 3, url: 'https://github.com/o/r/issues/3' }),
        row({ id: 'b', number: 1, url: 'https://github.com/o/r/issues/1' }),
        // A repeated URL (defensive) — the second occurrence is dropped.
        row({ id: 'a-dup', number: 3, url: 'https://github.com/o/r/issues/3' })
      ])
    )
    expect(options.map((o) => o.number)).toEqual([3, 1])
  })

  it('treats a missing issue state as open, but drops an issue with no number/url', () => {
    const options = deriveIssueOptions(
      table([
        row({ id: 'no-state', number: 5, url: 'https://github.com/o/r/issues/5', state: null }),
        row({ id: 'no-number', number: null, url: 'https://github.com/o/r/issues/9' }),
        row({ id: 'no-url', number: 7, url: null })
      ])
    )
    expect(options.map((o) => o.itemId)).toEqual(['no-state'])
  })

  it('carries the row repository through (a Project can span repos)', () => {
    const options = deriveIssueOptions(
      table([
        row({
          id: 'x',
          number: 4,
          url: 'https://github.com/other/repo/issues/4',
          repository: 'other/repo'
        })
      ])
    )
    expect(options[0].repository).toBe('other/repo')
  })

  it('filters a mixed Project to the server-resolved repository slug', () => {
    const options = deriveIssueOptions(
      table([
        row({ id: 'agentum', number: 1, repository: 'Mateo/Agentum' }),
        row({ id: 'xcode', number: 2, repository: ' mateo/xcode-theme ' }),
        row({ id: 'missing', number: 3, repository: null })
      ]),
      'MATEO/XCODE-THEME'
    )
    expect(options.map((option) => option.itemId)).toEqual(['xcode'])
  })
})

describe('isPickableIssueRow', () => {
  it('accepts an open issue and rejects PR / draft / closed', () => {
    expect(isPickableIssueRow(row({ itemType: 'ISSUE', state: 'OPEN' }))).toBe(true)
    expect(isPickableIssueRow(row({ itemType: 'PULL_REQUEST', state: 'OPEN' }))).toBe(false)
    expect(isPickableIssueRow(row({ itemType: 'DRAFT_ISSUE' }))).toBe(false)
    expect(isPickableIssueRow(row({ itemType: 'ISSUE', state: 'closed' }))).toBe(false)
  })
})

describe('buildBindPayload', () => {
  it('shapes a github-provider bind + an issue LinkedWorkItemSummary', () => {
    const [option] = deriveIssueOptions(
      table([
        row({
          id: 'i',
          number: 42,
          title: 'Add OAuth',
          url: 'https://github.com/o/r/issues/42'
        })
      ])
    )
    const bind = buildBindPayload(option)
    expect(bind.trackerProvider).toBe('github')
    expect(bind.trackerUrl).toBe('https://github.com/o/r/issues/42')
    expect(bind.summary).toEqual({
      type: 'issue',
      number: 42,
      title: 'Add OAuth',
      url: 'https://github.com/o/r/issues/42'
    })
  })
})

describe('resolvePickerProject', () => {
  const active = { owner: 'globalorg', ownerType: 'organization' as const, number: 7 }
  // A per-repo binding identity (spec 010's BoardBinding wire shape); extra
  // fields the real DTO carries are irrelevant to resolution.
  function binding(overrides: {
    owner?: string | null
    ownerType?: string | null
    number?: number | null
  }) {
    return {
      kind: 'resolved' as const,
      targetKey: 'repo-1:/repo',
      repositorySlug: 'repoorg/repo',
      binding: {
        projectOwner: overrides.owner === undefined ? 'repoorg' : overrides.owner,
        projectOwnerType: overrides.ownerType === undefined ? 'organization' : overrides.ownerType,
        projectNumber: overrides.number === undefined ? 3 : overrides.number
      }
    }
  }

  it('prefers the per-repo binding over the global activeProject', () => {
    expect(
      resolvePickerProject({
        binding: binding({}),
        activeProject: active,
        selectedGitRepo: true
      })
    ).toEqual({
      owner: 'repoorg',
      ownerType: 'organization',
      number: 3
    })
  })

  it('falls back to activeProject when there is no binding (spec 012, no regression)', () => {
    expect(
      resolvePickerProject({ binding: null, activeProject: active, selectedGitRepo: false })
    ).toEqual(active)
  })

  it('never borrows activeProject while a selected repo binding is unresolved', () => {
    for (const kind of ['loading', 'absent', 'failed'] as const) {
      expect(
        resolvePickerProject({
          binding: { kind, targetKey: 'repo-1:/repo' },
          activeProject: active,
          selectedGitRepo: true
        })
      ).toBeNull()
    }
  })

  it('falls back to activeProject when the binding is partial (missing owner or number)', () => {
    expect(
      resolvePickerProject({
        binding: binding({ owner: null }),
        activeProject: active,
        selectedGitRepo: false
      })
    ).toEqual(active)
    expect(
      resolvePickerProject({
        binding: binding({ number: null }),
        activeProject: active,
        selectedGitRepo: false
      })
    ).toEqual(active)
  })

  it('returns null with neither a binding nor an activeProject (honest empty state)', () => {
    expect(
      resolvePickerProject({ binding: null, activeProject: null, selectedGitRepo: false })
    ).toBeNull()
    expect(
      resolvePickerProject({
        binding: binding({ owner: null }),
        activeProject: null,
        selectedGitRepo: false
      })
    ).toBeNull()
  })

  it('normalizes the binding ownerType: only an exact "organization" stays org, else user', () => {
    expect(
      resolvePickerProject({
        binding: binding({ ownerType: 'organization' }),
        activeProject: null,
        selectedGitRepo: true
      })
        ?.ownerType
    ).toBe('organization')
    expect(
      resolvePickerProject({
        binding: binding({ ownerType: 'user' }),
        activeProject: null,
        selectedGitRepo: true
      })
        ?.ownerType
    ).toBe('user')
    // A legacy/garbled ownerType collapses to user rather than leaking a bad value.
    expect(
      resolvePickerProject({
        binding: binding({ ownerType: 'USER' }),
        activeProject: null,
        selectedGitRepo: true
      })
        ?.ownerType
    ).toBe('user')
    expect(
      resolvePickerProject({
        binding: binding({ ownerType: null }),
        activeProject: null,
        selectedGitRepo: true
      })
        ?.ownerType
    ).toBe('user')
  })

  it('handles a binding with number 0 as a complete identity (not a falsy miss)', () => {
    // projectNumber 0 is unusual but valid — the guard checks `!= null`, not
    // truthiness, so a #0 board still wins over the fallback.
    expect(
      resolvePickerProject({
        binding: binding({ number: 0 }),
        activeProject: active,
        selectedGitRepo: true
      })
    ).toEqual({ owner: 'repoorg', ownerType: 'organization', number: 0 })
  })
})

describe('deriveTrackerIssueViewModel', () => {
  it('uses configured Status order, keeps position, and puts No status last', () => {
    const statusField = {
      id: 'status',
      name: 'Status',
      kind: 'single-select' as const,
      dataType: 'SINGLE_SELECT' as const,
      options: [
        { id: 'todo', name: 'Todo', color: 'GRAY' },
        { id: 'doing', name: 'In progress', color: 'YELLOW' }
      ]
    }
    const todo = row({
      id: 'todo-row',
      number: 2,
      title: 'Second',
      url: 'https://github.com/o/r/issues/2'
    })
    todo.position = 2
    todo.fieldValuesByFieldId.status = {
      kind: 'single-select',
      fieldId: 'status',
      optionId: 'todo',
      name: 'Todo',
      color: 'GRAY'
    }
    const doing = row({
      id: 'doing-row',
      number: 1,
      title: 'First',
      url: 'https://github.com/o/r/issues/1'
    })
    doing.position = 1
    doing.fieldValuesByFieldId.status = {
      kind: 'single-select',
      fieldId: 'status',
      optionId: 'doing',
      name: 'In progress',
      color: 'YELLOW'
    }
    const none = row({
      id: 'none-row',
      number: 3,
      title: 'Unassigned',
      url: 'https://github.com/o/r/issues/3'
    })
    none.position = 0
    const value = table([none, doing, todo])
    value.selectedView.fields = [statusField]
    value.selectedView.groupByFields = [statusField]

    const view = deriveTrackerIssueViewModel(value)
    expect(view.groups.map((group) => group.label)).toEqual(['Todo', 'In progress', 'No status'])
    expect(view.groups.map((group) => group.color)).toEqual(['GRAY', 'YELLOW', null])
    expect(view.groups.flatMap((group) => group.options.map((option) => option.number))).toEqual([
      2, 1, 3
    ])
  })

  it('filters by title or exact issue number and exposes a stable project key', () => {
    const value = table([
      row({ id: 'a', number: 12, title: 'Refresh tracker' }),
      row({ id: 'b', number: 120, title: 'Unrelated' })
    ])
    expect(deriveTrackerIssueViewModel(value, 'refresh').options.map((item) => item.number)).toEqual([
      12
    ])
    expect(deriveTrackerIssueViewModel(value, '#12').options.map((item) => item.number)).toEqual([12])
    expect(pickerProjectKey({ owner: 'Acme', ownerType: 'organization', number: 7 })).toBe(
      'organization:acme:7'
    )
    expect(
      pickerScopeKey({
        targetKey: 'repo-a:/work',
        repositorySlug: ' Acme/Widgets ',
        project: { owner: 'Acme', ownerType: 'organization', number: 7 }
      })
    ).toBe('repo-a:/work:acme/widgets:organization:acme:7')
    expect(
      pickerScopeKey({
        targetKey: 'repo-b:/work',
        repositorySlug: 'acme/other',
        project: { owner: 'Acme', ownerType: 'organization', number: 7 }
      })
    ).not.toBe('repo-a:/work:acme/widgets:organization:acme:7')
  })

  it('filters groups and counts before rendering a mixed-repository Project', () => {
    const value = table([
      row({ id: 'a', number: 1, repository: 'acme/agentum' }),
      row({ id: 'x', number: 2, repository: 'ACME/XCODE-THEME' }),
      row({ id: 'none', number: 3, repository: null })
    ])
    const view = deriveTrackerIssueViewModel(value, '', 'acme/xcode-theme')
    expect(view.issueCount).toBe(1)
    expect(view.options.map((option) => option.itemId)).toEqual(['x'])
  })
})

describe('deriveTrackerBindCoords', () => {
  it('binds a GitHub issue by URL', () => {
    expect(
      deriveTrackerBindCoords({ type: 'issue', url: 'https://github.com/o/r/issues/42' })
    ).toEqual({ trackerProvider: 'github', trackerUrl: 'https://github.com/o/r/issues/42' })
  })

  it('binds a Linear item by identifier (url ignored)', () => {
    expect(
      deriveTrackerBindCoords({ type: 'issue', url: 'https://linear.app/x', linearIdentifier: 'ENG-9' })
    ).toEqual({ trackerProvider: 'linear', trackerUrl: 'ENG-9' })
  })

  it('binds nothing for a PR, a non-GitHub issue, or a missing item (fail-closed)', () => {
    expect(
      deriveTrackerBindCoords({ type: 'pr', url: 'https://github.com/o/r/pull/12' })
    ).toBeNull()
    expect(
      deriveTrackerBindCoords({ type: 'issue', url: 'https://gitlab.com/o/r/-/issues/3' })
    ).toBeNull()
    expect(deriveTrackerBindCoords(null)).toBeNull()
    expect(deriveTrackerBindCoords(undefined)).toBeNull()
  })
})
