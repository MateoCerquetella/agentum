import { describe, expect, it } from 'vitest'
import type { Repo, Worktree } from '../../../../shared/types'
import type { OperationalWorkspaceFact } from './operational-sidebar-model'
import {
  buildOperationalSidebarRows,
  formatOperationalShortAge
} from './operational-sidebar-model'

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
      ['blocked', 'working', 'ready', 'open', 'quiet'].map((id) => worktree(id)),
      {
        blocked: { status: 'permission' },
        working: { status: 'working' },
        ready: { status: 'done' },
        open: { status: 'active' },
        quiet: { status: 'inactive' }
      }
    )
    expect(rows.filter((row) => row.type === 'header').map((row) => [row.label, row.count])).toEqual([
      ['Needs You', 1],
      ['Active', 3],
      ['Settled', 1]
    ])
    const items = rows.filter((row) => row.type === 'item')
    expect(items.map((row) => row.worktree.id).sort()).toEqual([
      'blocked',
      'open',
      'quiet',
      'ready',
      'working'
    ])
    expect(items.map((row) => row.operationalMeta?.statusLabel)).toEqual([
      'Needs input',
      'Active',
      'Ready',
      'Working',
      'Settled'
    ])
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

  it('orders pinned first, then activity, and progressively discloses settled rows', () => {
    const trees = [
      worktree('old', { lastActivityAt: 10 }),
      worktree('new', { lastActivityAt: 30 }),
      worktree('pinned', { lastActivityAt: 1, isPinned: true }),
      worktree('middle', { lastActivityAt: 20 })
    ]
    const facts = Object.fromEntries(trees.map((tree) => [tree.id, { status: 'inactive' }]))
    const collapsed = build(trees, facts, { settledLimit: 3 })
    expect(collapsed.filter((row) => row.type === 'item').map((row) => row.worktree.id)).toEqual([
      'pinned',
      'new',
      'middle'
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

describe('formatOperationalShortAge', () => {
  it('formats compact ages and rejects missing or non-finite timestamps', () => {
    expect(formatOperationalShortAge(995_000, 1_000_000)).toBe('now')
    expect(formatOperationalShortAge(700_000, 1_000_000)).toBe('5m')
    expect(formatOperationalShortAge(undefined, 1_000_000)).toBeUndefined()
    expect(formatOperationalShortAge(Number.NaN, 1_000_000)).toBeUndefined()
  })
})
