import { describe, expect, it } from 'vitest'
import type { Repo, Worktree } from '@/shared/types'
import type { OperationalWorkspaceFact } from './operational-sidebar-model'
import {
  buildOperationalSidebarRows,
  formatOperationalShortAge,
  selectOperationalContinuation,
  selectOperationalStatusTimestamp
} from './operational-sidebar-model'
import { AGENT_STATUS_STALE_AFTER_MS } from '@/shared/agent-status-types'

function repo(id: string, displayName: string): Repo {
  return { id, displayName, path: `/tmp/${id}`, badgeColor: '#000', addedAt: 0 }
}

function worktree(id: string, overrides: Partial<Worktree> = {}): Worktree {
  return {
    id,
    repoId: 'r1',
    displayName: id,
    branch: `feature/${id}`,
    lastActivityAt: 0,
    isPinned: false,
    ...overrides
  } as Worktree
}

function build(worktrees: Worktree[], facts: Record<string, OperationalWorkspaceFact>, options = {}) {
  return buildOperationalSidebarRows({
    worktrees,
    repoMap: new Map([
      ['r1', repo('r1', 'Agentum')],
      ['r2', repo('r2', 'Freebee')]
    ]),
    factsByWorktreeId: new Map(Object.entries(facts)),
    now: 1_000_000,
    ...options
  })
}

describe('buildOperationalSidebarRows', () => {
  it('partitions every workspace once and emits full counts in fixed section order', () => {
    const rows = build(
      ['blocked', 'working', 'continue', 'done', 'open', 'quiet'].map((id) => worktree(id)),
      {
        blocked: { status: 'permission' },
        working: { status: 'working' },
        continue: {
          status: 'done',
          continuation: { stateTimestamp: 900_000, agentLabel: 'Codex' }
        },
        done: { status: 'done' },
        open: { status: 'active' },
        quiet: { status: 'inactive' }
      }
    )
    expect(rows.filter((row) => row.type === 'header').map((row) => [row.label, row.count])).toEqual([
      ['Needs You', 2],
      ['Active', 1],
      ['Settled', 3]
    ])
    const items = rows.filter((row) => row.type === 'item')
    expect(items.map((row) => row.worktree.id).sort()).toEqual([
      'blocked',
      'continue',
      'done',
      'open',
      'quiet',
      'working'
    ])
    expect(
      Object.fromEntries(
        items.map((row) => [row.worktree.id, row.operationalMeta?.statusLabel])
      )
    ).toEqual({
      blocked: 'Needs input',
      continue: 'Ready to continue',
      working: 'Working',
      done: 'Settled',
      open: 'Settled',
      quiet: 'Settled'
    })
  })

  it('prioritizes unseen completion over working, while explicit input remains first', () => {
    const rows = build(
      [worktree('parallel'), worktree('blocked')],
      {
        parallel: {
          status: 'working',
          agentLabel: 'Claude',
          stateTimestamp: 800_000,
          continuation: { stateTimestamp: 900_000, agentLabel: 'Codex' }
        },
        blocked: {
          status: 'permission',
          agentLabel: 'Claude',
          stateTimestamp: 950_000,
          continuation: { stateTimestamp: 990_000, agentLabel: 'Codex' }
        }
      }
    )
    const items = rows.filter((row) => row.type === 'item')
    const parallel = items.find((row) => row.worktree.id === 'parallel')
    const blocked = items.find((row) => row.worktree.id === 'blocked')

    expect(parallel?.operationalMeta).toMatchObject({
      section: 'needs-you',
      statusLabel: 'Ready to continue',
      agentLabel: 'Codex',
      stateTimestamp: 900_000
    })
    expect(blocked?.operationalMeta).toMatchObject({
      section: 'needs-you',
      statusLabel: 'Needs input',
      agentLabel: 'Claude',
      stateTimestamp: 950_000
    })
  })

  it('promotes the durable unread attention signal to Ready to continue', () => {
    const rows = build(
      [
        worktree('unread-idle', { isUnread: true, lastActivityAt: 880_000 }),
        worktree('unread-working', { isUnread: true, lastActivityAt: 700_000 }),
        worktree('read-idle', { isUnread: false, lastActivityAt: 600_000 })
      ],
      {
        'unread-idle': { status: 'inactive', agentLabel: 'Codex' },
        'unread-working': {
          status: 'working',
          agentLabel: 'Claude',
          stateTimestamp: 900_000
        },
        'read-idle': { status: 'inactive' }
      }
    )
    const headers = rows.filter((row) => row.type === 'header')
    const items = rows.filter((row) => row.type === 'item')

    expect(headers.map((row) => [row.label, row.count])).toEqual([
      ['Needs You', 2],
      ['Active', 0],
      ['Settled', 1]
    ])
    expect(
      items.find((row) => row.worktree.id === 'unread-idle')?.operationalMeta
    ).toMatchObject({
      section: 'needs-you',
      statusLabel: 'Ready to continue',
      agentLabel: 'Codex',
      stateTimestamp: 880_000
    })
    expect(
      items.find((row) => row.worktree.id === 'unread-working')?.operationalMeta
    ).toMatchObject({
      section: 'needs-you',
      statusLabel: 'Ready to continue',
      agentLabel: 'Claude',
      stateTimestamp: 900_000
    })
  })

  it('keeps explicit input ahead of unread continuation state', () => {
    const rows = build(
      [worktree('blocked-unread', { isUnread: true, lastActivityAt: 980_000 })],
      {
        'blocked-unread': {
          status: 'permission',
          agentLabel: 'Claude',
          stateTimestamp: 950_000
        }
      }
    )
    const item = rows.find((row) => row.type === 'item')

    expect(item?.operationalMeta).toMatchObject({
      section: 'needs-you',
      statusLabel: 'Needs input',
      stateTimestamp: 950_000
    })
  })

  it('treats generic open-surface liveness as settled, not active work', () => {
    const rows = build([worktree('open')], { open: { status: 'active' } })
    const item = rows.find((row) => row.type === 'item')

    expect(item?.operationalMeta).toMatchObject({
      section: 'settled',
      statusLabel: 'Settled',
      presentation: 'operational-settled'
    })
  })

  it('searches display name, branch, project, and visible agent label', () => {
    const trees = [
      worktree('alpha', { displayName: 'Payment cleanup' }),
      worktree('beta', { repoId: 'r2', branch: 'fix/marketing' }),
      worktree('gamma')
    ]
    const facts = {
      alpha: { status: 'working', agentLabel: 'Codex' },
      beta: { status: 'inactive', agentLabel: 'Cursor' },
      gamma: { status: 'inactive', agentLabel: 'Claude' }
    } satisfies Record<string, OperationalWorkspaceFact>
    for (const query of ['payment', 'marketing', 'freebee', 'claude']) {
      const items = build(trees, facts, { query }).filter((row) => row.type === 'item')
      expect(items).toHaveLength(1)
    }
  })

  it('searches the continuation agent that the card actually displays', () => {
    const facts = {
      handoff: {
        status: 'working',
        agentLabel: 'Claude',
        continuation: { stateTimestamp: 900_000, agentLabel: 'Codex' }
      }
    } satisfies Record<string, OperationalWorkspaceFact>

    expect(
      build([worktree('handoff')], facts, { query: 'codex' }).filter(
        (row) => row.type === 'item'
      )
    ).toHaveLength(1)
    expect(
      build([worktree('handoff')], facts, { query: 'claude' }).filter(
        (row) => row.type === 'item'
      )
    ).toHaveLength(0)
  })

  it('orders settled strictly by activity and progressively discloses settled rows', () => {
    const trees = [
      worktree('old', { lastActivityAt: 10 }),
      worktree('new', { lastActivityAt: 30 }),
      worktree('pinned', { lastActivityAt: 1, isPinned: true }),
      worktree('middle', { lastActivityAt: 20 })
    ]
    const facts: Record<string, OperationalWorkspaceFact> = Object.fromEntries(
      trees.map((tree) => [tree.id, { status: 'inactive' as const }])
    )
    const collapsed = build(trees, facts, { settledLimit: 3 })
    expect(collapsed.filter((row) => row.type === 'item').map((row) => row.worktree.id)).toEqual([
      'new',
      'middle',
      'old'
    ])
    expect(collapsed[collapsed.length - 1]).toMatchObject({
      type: 'operational-settled-disclosure',
      remainingCount: 1,
      expanded: false
    })
    const expanded = build(trees, facts, { settledLimit: 3, settledExpanded: true })
    expect(expanded.filter((row) => row.type === 'item')).toHaveLength(4)
    expect(expanded[expanded.length - 1]).toMatchObject({ expanded: true, remainingCount: 1 })
  })

  it('omits a rich state age when no matching winning signal timestamp is provided', () => {
    const rows = build(
      [worktree('watchdog', { lastActivityAt: 900_000 })],
      { watchdog: { status: 'permission' } }
    )
    const item = rows.find((row) => row.type === 'item')
    expect(item?.operationalMeta).toMatchObject({ statusLabel: 'Needs input' })
    expect(item?.operationalMeta?.relativeAge).toBeUndefined()
  })

  it('omits unavailable optional metadata and treats a missing fact as settled', () => {
    const rows = buildOperationalSidebarRows({
      worktrees: [worktree('plain', { repoId: 'missing', lastActivityAt: Number.NaN })],
      repoMap: new Map(),
      factsByWorktreeId: new Map(),
      now: 100
    })
    const item = rows.find((row) => row.type === 'item')
    expect(item?.operationalMeta).toMatchObject({ status: 'inactive', statusLabel: 'Settled' })
    expect(item?.operationalMeta?.projectName).toBeUndefined()
    expect(item?.operationalMeta?.relativeAge).toBeUndefined()
  })
})

describe('selectOperationalContinuation', () => {
  const entries = [
    {
      paneKey: 'tab-1:leaf-1',
      state: 'done' as const,
      stateStartedAt: 100,
      updatedAt: 150,
      agentType: 'codex'
    },
    {
      paneKey: 'tab-2:leaf-2',
      state: 'done' as const,
      stateStartedAt: 200,
      updatedAt: 250,
      agentType: 'claude'
    },
    {
      paneKey: 'tab-3:leaf-3',
      state: 'working' as const,
      stateStartedAt: 300,
      updatedAt: 350,
      agentType: 'gemini'
    }
  ]

  it('selects the newest unacknowledged current completion', () => {
    expect(selectOperationalContinuation(entries, {})).toEqual({
      stateTimestamp: 200,
      agentLabel: 'claude'
    })
  })

  it('falls back to an older unviewed completion and clears after all are viewed', () => {
    expect(
      selectOperationalContinuation(entries, {
        'tab-2:leaf-2': 200
      })
    ).toEqual({ stateTimestamp: 100, agentLabel: 'codex' })
    expect(
      selectOperationalContinuation(entries, {
        'tab-1:leaf-1': 101,
        'tab-2:leaf-2': 200
      })
    ).toBeUndefined()
  })
})

describe('formatOperationalShortAge', () => {
  it('formats compact ages and rejects missing or non-finite timestamps', () => {
    expect(formatOperationalShortAge(995_000, 1_000_000)).toBe('now')
    expect(formatOperationalShortAge(700_000, 1_000_000)).toBe('5m')
    expect(formatOperationalShortAge(undefined, 1_000_000)).toBeUndefined()
    expect(formatOperationalShortAge(Number.NaN, 1_000_000)).toBeUndefined()
  })
})

describe('selectOperationalStatusTimestamp', () => {
  it('uses the same urgent pane signal that won aggregate precedence', () => {
    const now = 2_000
    const entries = [
      { state: 'waiting' as const, stateStartedAt: 100, updatedAt: 200 },
      { state: 'working' as const, stateStartedAt: 900, updatedAt: 1_000 }
    ]

    expect(selectOperationalStatusTimestamp('permission', entries, now)).toBe(100)
    expect(selectOperationalStatusTimestamp('working', entries, now)).toBe(900)
  })

  it('omits timestamps when the winning aggregate status has no explicit matching pane', () => {
    expect(
      selectOperationalStatusTimestamp('permission', [
        { state: 'working', stateStartedAt: 900, updatedAt: 1_000 }
      ], 2_000)
    ).toBeUndefined()
  })

  it('omits fallback permission and done ages when same-state pane entries are stale', () => {
    const now = 4_000_000
    const staleUpdatedAt = now - AGENT_STATUS_STALE_AFTER_MS - 1

    expect(
      selectOperationalStatusTimestamp(
        'permission',
        [{ state: 'blocked', stateStartedAt: 100, updatedAt: staleUpdatedAt }],
        now
      )
    ).toBeUndefined()
    expect(
      selectOperationalStatusTimestamp(
        'done',
        [{ state: 'done', stateStartedAt: 200, updatedAt: staleUpdatedAt }],
        now
      )
    ).toBeUndefined()
  })
})
