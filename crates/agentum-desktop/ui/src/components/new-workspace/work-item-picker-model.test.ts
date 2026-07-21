import { describe, expect, it } from 'vitest'
import type {
  GitHubProjectRow,
  GitHubProjectTable
} from '../../shared/github-project-types'
import {
  buildBindPayload,
  deriveIssueOptions,
  deriveTrackerBindCoords,
  isPickableIssueRow,
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
      projectOwner: overrides.owner === undefined ? 'repoorg' : overrides.owner,
      projectOwnerType: overrides.ownerType === undefined ? 'organization' : overrides.ownerType,
      projectNumber: overrides.number === undefined ? 3 : overrides.number
    }
  }

  it('prefers the per-repo binding over the global activeProject', () => {
    expect(resolvePickerProject({ binding: binding({}), activeProject: active })).toEqual({
      owner: 'repoorg',
      ownerType: 'organization',
      number: 3
    })
  })

  it('falls back to activeProject when there is no binding (spec 012, no regression)', () => {
    expect(resolvePickerProject({ binding: null, activeProject: active })).toEqual(active)
    expect(resolvePickerProject({ binding: undefined, activeProject: active })).toEqual(active)
  })

  it('falls back to activeProject when the binding is partial (missing owner or number)', () => {
    expect(
      resolvePickerProject({ binding: binding({ owner: null }), activeProject: active })
    ).toEqual(active)
    expect(
      resolvePickerProject({ binding: binding({ number: null }), activeProject: active })
    ).toEqual(active)
  })

  it('returns null with neither a binding nor an activeProject (honest empty state)', () => {
    expect(resolvePickerProject({ binding: null, activeProject: null })).toBeNull()
    expect(
      resolvePickerProject({ binding: binding({ owner: null }), activeProject: null })
    ).toBeNull()
  })

  it('normalizes the binding ownerType: only an exact "organization" stays org, else user', () => {
    expect(
      resolvePickerProject({ binding: binding({ ownerType: 'organization' }), activeProject: null })
        ?.ownerType
    ).toBe('organization')
    expect(
      resolvePickerProject({ binding: binding({ ownerType: 'user' }), activeProject: null })
        ?.ownerType
    ).toBe('user')
    // A legacy/garbled ownerType collapses to user rather than leaking a bad value.
    expect(
      resolvePickerProject({ binding: binding({ ownerType: 'USER' }), activeProject: null })
        ?.ownerType
    ).toBe('user')
    expect(
      resolvePickerProject({ binding: binding({ ownerType: null }), activeProject: null })
        ?.ownerType
    ).toBe('user')
  })

  it('handles a binding with number 0 as a complete identity (not a falsy miss)', () => {
    // projectNumber 0 is unusual but valid — the guard checks `!= null`, not
    // truthiness, so a #0 board still wins over the fallback.
    expect(
      resolvePickerProject({ binding: binding({ number: 0 }), activeProject: active })
    ).toEqual({ owner: 'repoorg', ownerType: 'organization', number: 0 })
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
