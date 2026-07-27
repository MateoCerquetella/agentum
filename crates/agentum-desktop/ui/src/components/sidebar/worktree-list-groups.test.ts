/* eslint-disable max-lines -- Why: row-builder tests keep grouping, pinning, and lineage ordering cases together so expected row contracts stay comparable. */
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  ALL_GROUP_META,
  buildRows,
  getGroupKeyForWorktree,
  getGroupKeysForWorktree,
  getHostHeaderKey,
  getLineageGroupKey,
  getLineageRenderInfo,
  getPRGroupKey,
  getProjectGroupOrdering,
  groupRowsByHost,
  hasTmuxForHost,
  hostKeysWithOpenTmux,
  LOCAL_HOST_KEY,
  hostKeyForRepo,
  pickWorktreeForHost,
  PINNED_GROUP_KEY,
  sidebarHostStatus,
  type HostTmuxSession,
  type Row,
  type SidebarHost
} from './worktree-list-groups'
import type {
  DetectedWorktree,
  Repo,
  ProjectGroup,
  Worktree,
  WorktreeLineage
} from '../../../../shared/types'

const repo: Repo = {
  id: 'repo-1',
  path: '/tmp/agentum',
  displayName: 'agentum',
  badgeColor: '#000000',
  addedAt: 0
}

const worktree: Worktree = {
  id: 'wt-1',
  repoId: repo.id,
  path: '/tmp/agentum-feature',
  branch: 'refs/heads/feature/super-critical',
  head: 'abc123',
  isBare: false,
  isMainWorktree: false,
  linkedIssue: null,
  linkedPR: null,
  linkedLinearIssue: null,
  isArchived: false,
  comment: '',
  isUnread: false,
  isPinned: false,
  displayName: 'feature/super-critical',
  sortOrder: 0,
  lastActivityAt: 0
}

const repoMap = new Map([[repo.id, repo]])

describe('hasTmuxForHost', () => {
  // server host UUID → sidebar host key, as hydrateHosts builds it.
  const serverHostIdToHostKey = new Map<string, string>([
    ['local-uuid', 'local'],
    ['omarchy-uuid', 'ssh:conn-omarchy']
  ])

  const running = (over: Partial<HostTmuxSession>): HostTmuxSession => ({
    status: 'running',
    tmux_target: 'agentum-feature-claude-abc',
    host_id: null,
    ...over
  })

  it('includes a running session with a tmux_target on the matching host', () => {
    const sessions = [running({ host_id: 'omarchy-uuid' })]
    expect(hasTmuxForHost(sessions, 'ssh:conn-omarchy', serverHostIdToHostKey)).toBe(true)
  })

  it('buckets a null host_id under the local host', () => {
    const sessions = [running({ host_id: null })]
    expect(hasTmuxForHost(sessions, 'local', serverHostIdToHostKey)).toBe(true)
    expect(hasTmuxForHost(sessions, 'ssh:conn-omarchy', serverHostIdToHostKey)).toBe(false)
  })

  it('excludes a session whose tmux_target is null or absent (local PTY / crashed)', () => {
    expect(hasTmuxForHost([running({ tmux_target: null })], 'local', serverHostIdToHostKey)).toBe(
      false
    )
    expect(
      hasTmuxForHost([running({ tmux_target: undefined })], 'local', serverHostIdToHostKey)
    ).toBe(false)
  })

  it('excludes a non-running session even with a tmux_target', () => {
    expect(
      hasTmuxForHost([running({ status: 'stopped' })], 'local', serverHostIdToHostKey)
    ).toBe(false)
    expect(
      hasTmuxForHost([running({ status: 'crashed' })], 'local', serverHostIdToHostKey)
    ).toBe(false)
  })

  it('excludes a tmux session that belongs to a different host', () => {
    const sessions = [running({ host_id: 'omarchy-uuid' })]
    expect(hasTmuxForHost(sessions, 'local', serverHostIdToHostKey)).toBe(false)
  })

  it('returns false for an empty session list', () => {
    expect(hasTmuxForHost([], 'local', serverHostIdToHostKey)).toBe(false)
  })
})

describe('hostKeysWithOpenTmux', () => {
  // tabId → worktreeId, as WorktreeList assembles from tabsByWorktree.
  const tabIdToWorktreeId = new Map<string, string>([
    ['tab-local', 'wt-local'],
    ['tab-remote', 'wt-remote']
  ])
  // worktreeId → host key, as WorktreeList assembles from worktree → repo.
  const worktreeIdToHostKey = new Map<string, string>([
    ['wt-local', 'local'],
    ['wt-remote', 'ssh:conn-omarchy']
  ])

  it('maps a local tab pane key to the local host', () => {
    const out = hostKeysWithOpenTmux({ 'tab-local:leaf-1': true }, tabIdToWorktreeId, worktreeIdToHostKey)
    expect(out).toEqual(new Set(['local']))
  })

  it('maps a remote tab pane key to its ssh host key', () => {
    const out = hostKeysWithOpenTmux(
      { 'tab-remote:leaf-1': true },
      tabIdToWorktreeId,
      worktreeIdToHostKey
    )
    expect(out).toEqual(new Set(['ssh:conn-omarchy']))
  })

  it('collapses multiple open panes on the same host into one key', () => {
    const out = hostKeysWithOpenTmux(
      { 'tab-local:leaf-1': true, 'tab-local:leaf-2': true },
      tabIdToWorktreeId,
      worktreeIdToHostKey
    )
    expect(out).toEqual(new Set(['local']))
  })

  it('returns both hosts when open tmux panes span local and remote', () => {
    const out = hostKeysWithOpenTmux(
      { 'tab-local:leaf-1': true, 'tab-remote:leaf-9': true },
      tabIdToWorktreeId,
      worktreeIdToHostKey
    )
    expect(out).toEqual(new Set(['local', 'ssh:conn-omarchy']))
  })

  it('skips a pane key whose tab no longer exists (closed/persisted leftover)', () => {
    const out = hostKeysWithOpenTmux({ 'tab-gone:leaf-1': true }, tabIdToWorktreeId, worktreeIdToHostKey)
    expect(out).toEqual(new Set())
  })

  it('skips a pane key whose worktree has no host mapping', () => {
    const out = hostKeysWithOpenTmux(
      { 'tab-orphan:leaf-1': true },
      new Map([['tab-orphan', 'wt-unmapped']]),
      worktreeIdToHostKey
    )
    expect(out).toEqual(new Set())
  })

  it('returns an empty set for an empty tmux map', () => {
    expect(hostKeysWithOpenTmux({}, tabIdToWorktreeId, worktreeIdToHostKey)).toEqual(new Set())
  })
})

describe('hostKeyForRepo', () => {
  it('buckets a local repo (no host association) under the synthetic local key', () => {
    expect(hostKeyForRepo(repo)).toBe(LOCAL_HOST_KEY)
    expect(hostKeyForRepo(repo)).toBe('local')
  })

  it('buckets a repo with a connectionId under the ssh: prefixed key', () => {
    expect(hostKeyForRepo({ ...repo, connectionId: 'ssh-1' })).toBe('ssh:ssh-1')
  })

  it('returns the local key for a repo with no connectionId', () => {
    expect(hostKeyForRepo({ ...repo, connectionId: undefined })).toBe(LOCAL_HOST_KEY)
  })

  it('returns the local key when called with undefined', () => {
    expect(hostKeyForRepo(undefined)).toBe(LOCAL_HOST_KEY)
  })

  it('groups multiple repos on the same host under one key, distinct from other hosts', () => {
    const a = { ...repo, id: 'a', connectionId: 'ssh-1' }
    const b = { ...repo, id: 'b', connectionId: 'ssh-1' }
    const c = { ...repo, id: 'c', connectionId: 'ssh-2' }
    expect(hostKeyForRepo(a)).toBe(hostKeyForRepo(b))
    expect(hostKeyForRepo(a)).not.toBe(hostKeyForRepo(c))
  })
})

function makeDetectedWorktree(overrides: Partial<DetectedWorktree> = {}): DetectedWorktree {
  return {
    ...worktree,
    id: overrides.id ?? `${repo.id}::/tmp/${overrides.displayName ?? 'hidden'}`,
    path: overrides.path ?? `/tmp/${overrides.displayName ?? 'hidden'}`,
    displayName: overrides.displayName ?? 'hidden',
    visible: false,
    selectedCheckout: false,
    ownership: 'external',
    ...overrides
  }
}

describe('getPRGroupKey', () => {
  it('puts merged PRs in the done group', () => {
    const prCache = {
      'repo-1::feature/super-critical': {
        data: { state: 'merged' }
      }
    }

    expect(getPRGroupKey(worktree, repoMap, prCache)).toBe('done')
  })

  it('prefers repo-scoped PR status over stale legacy path-scoped status', () => {
    const prCache = {
      '/tmp/agentum::feature/super-critical': {
        data: { state: 'closed' }
      },
      'repo-1::feature/super-critical': {
        data: { state: 'merged' }
      }
    }

    expect(getPRGroupKey(worktree, repoMap, prCache)).toBe('done')
  })

  it('falls back to legacy path-scoped PR status when no repo-scoped entry exists', () => {
    const prCache = {
      '/tmp/agentum::feature/super-critical': {
        data: { state: 'closed' }
      }
    }

    expect(getPRGroupKey(worktree, repoMap, prCache)).toBe('closed')
  })

  it('does not fall back to local PR cache while runtime scoped data is loading', () => {
    const prCache = {
      'repo-1::feature/super-critical': {
        data: { state: 'merged' }
      }
    }

    expect(
      getPRGroupKey(worktree, repoMap, prCache, {
        activeRuntimeEnvironmentId: 'env-1'
      } as never)
    ).toBe('in-progress')
  })

  it('uses SSH-scoped PR cache entries instead of local entries for SSH repos', () => {
    const sshRepo = { ...repo, connectionId: 'ssh-1' }
    const sshRepoMap = new Map([[sshRepo.id, sshRepo]])
    const prCache = {
      'repo-1::feature/super-critical': {
        data: { state: 'merged' }
      },
      'ssh:ssh-1::repo-1::feature/super-critical': {
        data: { state: 'closed' }
      }
    }

    expect(getPRGroupKey(worktree, sshRepoMap, prCache)).toBe('closed')
  })
})

describe('getGroupKeyForWorktree', () => {
  it('returns the all group key for the ungrouped mode', () => {
    expect(getGroupKeyForWorktree('none', worktree, repoMap, null)).toBe('all')
  })

  it('returns a workspace-status key only in status grouping mode', () => {
    expect(getGroupKeyForWorktree('workspace-status', worktree, repoMap, null)).toBe(
      'workspace-status:in-progress'
    )
  })
})

describe('buildRows with pinned worktrees', () => {
  const pinned = { ...worktree, id: 'wt-pinned', isPinned: true, displayName: 'pinned-feature' }
  const unpinned1 = { ...worktree, id: 'wt-1', displayName: 'alpha' }
  const unpinned2 = { ...worktree, id: 'wt-2', displayName: 'beta' }

  it('emits Pinned and All headers in groupBy none', () => {
    const rows = buildRows('none', [unpinned1, pinned, unpinned2], repoMap, null, new Set())
    expect(rows[0]).toMatchObject({ type: 'header', key: 'pinned', label: 'Pinned', count: 1 })
    expect(rows[1]).toMatchObject({ type: 'item', worktree: { id: 'wt-pinned' } })
    expect(rows[2]).toMatchObject({ type: 'header', key: 'all', label: 'All', count: 2 })
    expect(rows[2]).toMatchObject({ type: 'header', icon: ALL_GROUP_META.icon })
  })

  it('groups all worktrees under All in groupBy none', () => {
    const rows = buildRows('none', [unpinned1, unpinned2], repoMap, null, new Set())

    expect(rows).toMatchObject([
      { type: 'header', key: 'all', label: 'All', count: 2 },
      { type: 'item', worktree: { id: 'wt-1' } },
      { type: 'item', worktree: { id: 'wt-2' } }
    ])
  })

  it('keeps pinned worktrees above the All group', () => {
    const rows = buildRows('none', [unpinned1, pinned, unpinned2], repoMap, null, new Set())

    expect(rows).toMatchObject([
      { type: 'header', key: 'pinned', count: 1 },
      { type: 'item', worktree: { id: 'wt-pinned' } },
      { type: 'header', key: 'all', count: 2 },
      { type: 'item', worktree: { id: 'wt-1' } },
      { type: 'item', worktree: { id: 'wt-2' } }
    ])
  })

  it('collapses the All group in groupBy none', () => {
    const rows = buildRows('none', [unpinned1, pinned, unpinned2], repoMap, null, new Set(['all']))

    expect(rows).toMatchObject([
      { type: 'header', key: 'pinned', count: 1 },
      { type: 'item', worktree: { id: 'wt-pinned' } },
      { type: 'header', key: 'all', count: 2 }
    ])
  })

  it('emits status headers for unpinned worktrees in groupBy workspace-status', () => {
    const rows = buildRows(
      'workspace-status',
      [unpinned1, pinned, unpinned2],
      repoMap,
      null,
      new Set()
    )
    expect(rows[2]).toMatchObject({
      type: 'header',
      key: 'workspace-status:in-progress',
      label: 'In progress',
      count: 2
    })
    expect(rows[3]).toMatchObject({ type: 'item', worktree: { id: 'wt-1' } })
    expect(rows[4]).toMatchObject({ type: 'item', worktree: { id: 'wt-2' } })
  })

  it('excludes pinned items from regular groups in pr-status mode', () => {
    const rows = buildRows('pr-status', [unpinned1, pinned], repoMap, null, new Set())
    const pinnedHeader = rows.find((r) => r.type === 'header' && r.key === 'pinned')
    expect(pinnedHeader).toBeDefined()
    const prGroup = rows.filter((r) => r.type === 'header' && r.key.startsWith('pr:'))
    for (const header of prGroup) {
      if (header.type === 'header') {
        expect(header.count).toBe(1)
      }
    }
  })

  it('omits empty pinned sections in groupBy workspace-status', () => {
    const rows = buildRows('workspace-status', [unpinned1, unpinned2], repoMap, null, new Set())
    expect(rows[0]).toMatchObject({
      type: 'header',
      key: 'workspace-status:in-progress',
      label: 'In progress',
      count: 2
    })
    expect(rows[1]).toMatchObject({ type: 'item', worktree: { id: 'wt-1' } })
    expect(rows[2]).toMatchObject({ type: 'item', worktree: { id: 'wt-2' } })
  })

  it('collapses pinned group when in collapsedGroups', () => {
    const rows = buildRows(
      'workspace-status',
      [pinned, unpinned1],
      repoMap,
      null,
      new Set(['pinned'])
    )
    expect(rows[0]).toMatchObject({ type: 'header', key: 'pinned' })
    expect(rows[1]).toMatchObject({ type: 'header', key: 'workspace-status:in-progress' })
    expect(rows[2]).toMatchObject({ type: 'item', worktree: { id: 'wt-1' } })
  })

  it('does not emit empty status sections when all worktrees are pinned', () => {
    const allPinned = { ...unpinned1, isPinned: true }
    const rows = buildRows('workspace-status', [pinned, allPinned], repoMap, null, new Set())
    expect(rows.filter((r) => r.type === 'header')).toHaveLength(1)
    expect(rows[0]).toMatchObject({ type: 'header', key: 'pinned', count: 2 })
  })

  it('preserves repo display casing in group labels', () => {
    const lowercaseRepo = { ...repo, displayName: 'c15t' }
    const rows = buildRows('repo', [worktree], new Map([[repo.id, lowercaseRepo]]), null, new Set())

    expect(rows[0]).toMatchObject({ type: 'header', label: 'c15t' })
  })

  it('emits an imported worktrees card at the top of repo-group rows', () => {
    const hidden = [
      makeDetectedWorktree({ id: 'hidden-1', displayName: 'payments-refactor' }),
      makeDetectedWorktree({ id: 'hidden-2', displayName: 'auth-cache-debug' }),
      makeDetectedWorktree({ id: 'hidden-3', displayName: 'legacy-oauth-fix' })
    ]
    const rows = buildRows(
      'repo',
      [worktree],
      repoMap,
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      {},
      new Map([[worktree.id, worktree]]),
      false,
      undefined,
      [],
      new Set(),
      new Map([[repo.id, { repo, hiddenWorktrees: hidden }]])
    )

    expect(rows).toMatchObject([
      { type: 'header', key: 'repo:repo-1' },
      {
        type: 'imported-worktrees-card',
        key: 'imported-worktrees-card:repo-group:repo-1',
        placement: 'repo-group',
        repo: { id: 'repo-1' },
        hiddenWorktrees: [{ id: 'hidden-1' }, { id: 'hidden-2' }, { id: 'hidden-3' }]
      },
      { type: 'item', worktree: { id: 'wt-1' } }
    ])
  })

  it('suppresses the repo-group imported worktrees card when the repo group is collapsed', () => {
    const rows = buildRows(
      'repo',
      [worktree],
      repoMap,
      null,
      new Set(['repo:repo-1']),
      undefined,
      undefined,
      undefined,
      {},
      new Map([[worktree.id, worktree]]),
      false,
      undefined,
      [],
      new Set(),
      new Map([[repo.id, { repo, hiddenWorktrees: [makeDetectedWorktree()] }]])
    )

    expect(rows).toMatchObject([{ type: 'header', key: 'repo:repo-1' }])
  })

  it('emits a repo header and imported worktrees card when no visible worktree rows remain', () => {
    const rows = buildRows(
      'repo',
      [],
      repoMap,
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      {},
      new Map(),
      false,
      undefined,
      [],
      new Set(),
      new Map([[repo.id, { repo, hiddenWorktrees: [makeDetectedWorktree()] }]])
    )

    expect(rows).toMatchObject([
      { type: 'header', key: 'repo:repo-1', count: 0 },
      {
        type: 'imported-worktrees-card',
        key: 'imported-worktrees-card:repo-group:repo-1',
        placement: 'repo-group'
      }
    ])
  })

  it('does not emit unpinned imported worktree cards outside repo grouping', () => {
    const rows = buildRows(
      'workspace-status',
      [worktree],
      repoMap,
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      {},
      new Map([[worktree.id, worktree]]),
      false,
      undefined,
      [],
      new Set(),
      new Map([[repo.id, { repo, hiddenWorktrees: [makeDetectedWorktree()] }]])
    )

    expect(rows.some((row) => row.type === 'imported-worktrees-card')).toBe(false)
  })

  it('emits pinned-only imported worktree fallback cards after the repo final pinned row', () => {
    const repoTwo: Repo = { ...repo, id: 'repo-2', displayName: 'auth-service' }
    const pinnedOneA = { ...worktree, id: 'repo-1-pinned-a', isPinned: true }
    const pinnedTwo = {
      ...worktree,
      id: 'repo-2-pinned',
      repoId: repoTwo.id,
      isPinned: true,
      displayName: 'auth-main'
    }
    const pinnedOneB = { ...worktree, id: 'repo-1-pinned-b', isPinned: true }
    const rows = buildRows(
      'repo',
      [pinnedOneA, pinnedTwo, pinnedOneB],
      new Map([
        [repo.id, repo],
        [repoTwo.id, repoTwo]
      ]),
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      {},
      new Map([
        [pinnedOneA.id, pinnedOneA],
        [pinnedTwo.id, pinnedTwo],
        [pinnedOneB.id, pinnedOneB]
      ]),
      false,
      undefined,
      [],
      new Set(),
      new Map([
        [repo.id, { repo, hiddenWorktrees: [makeDetectedWorktree({ id: 'hidden-one' })] }],
        [
          repoTwo.id,
          {
            repo: repoTwo,
            hiddenWorktrees: [makeDetectedWorktree({ id: 'hidden-two', repoId: repoTwo.id })]
          }
        ]
      ])
    )

    expect(rows).toMatchObject([
      { type: 'header', key: 'pinned', count: 3 },
      { type: 'item', worktree: { id: 'repo-1-pinned-a' } },
      { type: 'item', worktree: { id: 'repo-2-pinned' } },
      {
        type: 'imported-worktrees-card',
        key: 'imported-worktrees-card:pinned-fallback:repo-2',
        placement: 'pinned-fallback'
      },
      { type: 'item', worktree: { id: 'repo-1-pinned-b' } },
      {
        type: 'imported-worktrees-card',
        key: 'imported-worktrees-card:pinned-fallback:repo-1',
        placement: 'pinned-fallback'
      }
    ])
  })

  it('suppresses pinned imported worktree fallback when the repo has visible unpinned rows', () => {
    const pinnedWorktree = { ...worktree, id: 'wt-pinned', isPinned: true }
    const rows = buildRows(
      'repo',
      [pinnedWorktree, worktree],
      repoMap,
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      {},
      new Map([
        [pinnedWorktree.id, pinnedWorktree],
        [worktree.id, worktree]
      ]),
      false,
      undefined,
      [],
      new Set(),
      new Map([[repo.id, { repo, hiddenWorktrees: [makeDetectedWorktree()] }]])
    )

    expect(rows.filter((row) => row.type === 'imported-worktrees-card')).toMatchObject([
      { placement: 'repo-group' }
    ])
  })

  it('suppresses pinned imported worktree fallback when Pinned is collapsed', () => {
    const pinnedWorktree = { ...worktree, id: 'wt-pinned', isPinned: true }
    const rows = buildRows(
      'repo',
      [pinnedWorktree],
      repoMap,
      null,
      new Set(['pinned']),
      undefined,
      undefined,
      undefined,
      {},
      new Map([[pinnedWorktree.id, pinnedWorktree]]),
      false,
      undefined,
      [],
      new Set(),
      new Map([[repo.id, { repo, hiddenWorktrees: [makeDetectedWorktree()] }]])
    )

    expect(rows).toMatchObject([{ type: 'header', key: 'pinned' }])
  })

  it('groups folder-mode workspaces under their folder name', () => {
    const folderRepo: Repo = {
      ...repo,
      id: 'folder-1',
      path: '/tmp/design-assets',
      displayName: 'design-assets',
      kind: 'folder'
    }
    const folderWorktree: Worktree = {
      ...worktree,
      id: 'folder-1::/tmp/design-assets',
      repoId: folderRepo.id,
      path: folderRepo.path,
      branch: '',
      displayName: folderRepo.displayName,
      isMainWorktree: true
    }
    const rows = buildRows(
      'repo',
      [folderWorktree],
      new Map([[folderRepo.id, folderRepo]]),
      null,
      new Set()
    )

    expect(rows[0]).toMatchObject({
      type: 'header',
      key: 'repo:folder-1',
      label: 'design-assets',
      count: 1,
      repo: folderRepo
    })
    expect(rows[1]).toMatchObject({ type: 'item', worktree: { id: folderWorktree.id } })
  })

  it('emits assigned workspace statuses as sections in groupBy workspace-status', () => {
    const review = { ...worktree, id: 'wt-review', workspaceStatus: 'in-review' as const }
    const rows = buildRows('workspace-status', [review], repoMap, null, new Set())

    expect(
      rows
        .filter((r) => r.type === 'header')
        .map((r) => ({ key: r.key, label: r.label, count: r.count }))
    ).toEqual([{ key: 'workspace-status:in-review', label: 'In review', count: 1 }])
  })

  it('uses customized workspace status labels and order', () => {
    const customStatuses = [
      { id: 'blocked', label: 'Blocked' },
      { id: 'todo', label: 'Ready' },
      { id: 'in-progress', label: 'Doing' }
    ]
    const blocked = { ...worktree, id: 'wt-blocked', workspaceStatus: 'blocked' }
    const doing = { ...worktree, id: 'wt-doing', workspaceStatus: 'in-progress' }
    const rows = buildRows(
      'workspace-status',
      [doing, blocked],
      repoMap,
      null,
      new Set(),
      undefined,
      customStatuses
    )

    expect(
      rows
        .filter((r) => r.type === 'header')
        .map((r) => ({ key: r.key, label: r.label, count: r.count }))
    ).toEqual([
      { key: 'workspace-status:blocked', label: 'Blocked', count: 1 },
      { key: 'workspace-status:in-progress', label: 'Doing', count: 1 }
    ])
  })
})

describe('buildRows project grouping order', () => {
  const repoA: Repo = { ...repo, id: 'repo-a', displayName: 'alpha' }
  const repoB: Repo = { ...repo, id: 'repo-b', displayName: 'beta' }
  const repoC: Repo = { ...repo, id: 'repo-c', displayName: 'gamma' }
  const map = new Map([
    [repoA.id, repoA],
    [repoB.id, repoB],
    [repoC.id, repoC]
  ])
  const wA: Worktree = { ...worktree, id: 'wt-a', repoId: repoA.id, displayName: 'a' }
  const wAStale: Worktree = { ...worktree, id: 'wt-a-stale', repoId: repoA.id, displayName: 'a2' }
  const wB: Worktree = { ...worktree, id: 'wt-b', repoId: repoB.id, displayName: 'b' }
  const wC: Worktree = { ...worktree, id: 'wt-c', repoId: repoC.id, displayName: 'c' }

  it('orders repo headers by explicit repoOrder, not first-encounter', () => {
    // Worktree stream encounters in order C, A, B — but repoOrder says B, A, C.
    const repoOrder = new Map([
      [repoB.id, 0],
      [repoA.id, 1],
      [repoC.id, 2]
    ])
    const rows = buildRows('repo', [wC, wA, wB], map, null, new Set(), repoOrder)
    const headerKeys = rows.filter((r) => r.type === 'header').map((r) => r.key)
    expect(headerKeys).toEqual(['repo:repo-b', 'repo:repo-a', 'repo:repo-c'])
  })

  it('places unknown repo ids last and sorts them by label', () => {
    // Only repoB is in repoOrder; repoA and repoC fall through to label sort.
    const repoOrder = new Map([[repoB.id, 0]])
    const rows = buildRows('repo', [wC, wA, wB], map, null, new Set(), repoOrder)
    const headerKeys = rows.filter((r) => r.type === 'header').map((r) => r.key)
    expect(headerKeys).toEqual(['repo:repo-b', 'repo:repo-a', 'repo:repo-c'])
  })

  it('orders repo headers by first encounter when caller uses visible worktree order', () => {
    // Caller already sorted worktrees by recency: C is freshest, then A, then B.
    // Even though repoOrder pins B, A, C, dynamic sorts must follow the freshest
    // worktree out of each repo so a just-active worktree's parent group
    // bubbles to the top of the sidebar.
    const repoOrder = new Map([
      [repoB.id, 0],
      [repoA.id, 1],
      [repoC.id, 2]
    ])
    const rows = buildRows(
      'repo',
      [wC, wA, wB],
      map,
      null,
      new Set(),
      repoOrder,
      undefined,
      'visible-worktree-order'
    )
    const headerKeys = rows.filter((r) => r.type === 'header').map((r) => r.key)
    expect(headerKeys).toEqual(['repo:repo-c', 'repo:repo-a', 'repo:repo-b'])
  })

  it('orders repo headers by each repo highest-ranked visible child', () => {
    const repoOrder = new Map([
      [repoB.id, 0],
      [repoA.id, 1],
      [repoC.id, 2]
    ])
    const rows = buildRows(
      'repo',
      [wA, wB, wAStale, wC],
      map,
      null,
      new Set(),
      repoOrder,
      undefined,
      'visible-worktree-order'
    )

    expect(rows).toMatchObject([
      { type: 'header', key: 'repo:repo-a' },
      { type: 'item', worktree: { id: 'wt-a' } },
      { type: 'item', worktree: { id: 'wt-a-stale' } },
      { type: 'header', key: 'repo:repo-b' },
      { type: 'item', worktree: { id: 'wt-b' } },
      { type: 'header', key: 'repo:repo-c' },
      { type: 'item', worktree: { id: 'wt-c' } }
    ])
  })

  it('keeps the main workspace first inside its project group', () => {
    const main = {
      ...wA,
      id: 'wt-a-main',
      displayName: 'main',
      isMainWorktree: true
    }
    const freshChild = {
      ...wA,
      id: 'wt-a-fresh-child',
      displayName: 'fresh-child',
      isMainWorktree: false
    }
    const rows = buildRows(
      'repo',
      [freshChild, wB, main],
      map,
      null,
      new Set(),
      undefined,
      undefined,
      'visible-worktree-order'
    )

    expect(rows).toMatchObject([
      { type: 'header', key: 'repo:repo-a' },
      { type: 'item', worktree: { id: 'wt-a-main' } },
      { type: 'item', worktree: { id: 'wt-a-fresh-child' } },
      { type: 'header', key: 'repo:repo-b' },
      { type: 'item', worktree: { id: 'wt-b' } }
    ])
  })

  it('keeps repoOrder for manual project group ordering', () => {
    const repoOrder = new Map([
      [repoB.id, 0],
      [repoA.id, 1],
      [repoC.id, 2]
    ])
    const rows = buildRows('repo', [wC, wA, wB], map, null, new Set(), repoOrder)
    const headerKeys = rows.filter((r) => r.type === 'header').map((r) => r.key)
    expect(headerKeys).toEqual(['repo:repo-b', 'repo:repo-a', 'repo:repo-c'])
  })

  it('builds rows for a very large repo-group list', () => {
    const count = 130_000
    const repos = new Map<string, Repo>()
    const worktrees = Array.from({ length: count }, (_, index) => {
      const repoId = `repo-${index}`
      repos.set(repoId, { ...repo, id: repoId, displayName: `repo ${index}` })
      return { ...worktree, id: `wt-${index}`, repoId, displayName: `workspace ${index}` }
    })

    const rows = buildRows('repo', worktrees, repos, null, new Set())

    expect(rows).toHaveLength(count * 2)
    expect(rows[0]).toMatchObject({ type: 'header', key: 'repo:repo-0' })
    expect(rows.at(-1)).toMatchObject({ type: 'item', worktree: { id: 'wt-129999' } })
  })
})

describe('getProjectGroupOrdering', () => {
  it.each([
    ['repo', 'recent', 'visible-worktree-order'],
    ['repo', 'smart', 'visible-worktree-order'],
    ['repo', 'name', 'manual'],
    ['repo', 'repo', 'manual'],
    ['none', 'recent', 'manual'],
    ['workspace-status', 'recent', 'manual'],
    ['pr-status', 'recent', 'manual']
  ] as const)('uses %s/%s -> %s', (groupBy, sortBy, expected) => {
    expect(getProjectGroupOrdering(groupBy, sortBy)).toBe(expected)
  })
})

describe('project groups', () => {
  it('keeps empty project groups visible in project grouping mode', () => {
    const group: ProjectGroup = {
      id: 'group-1',
      name: 'Platform',
      parentPath: null,
      parentGroupId: null,
      createdFrom: 'manual',
      tabOrder: 0,
      isCollapsed: false,
      color: null,
      createdAt: 1,
      updatedAt: 1
    }

    const rows = buildRows(
      'repo',
      [],
      new Map(),
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      {},
      new Map(),
      false,
      undefined,
      [group]
    )

    expect(rows).toEqual([
      expect.objectContaining({
        type: 'header',
        key: 'project-group:group-1',
        label: 'Platform',
        count: 0,
        projectGroup: group
      })
    ])
  })

  it('counts grouped repos before their visible worktrees are loaded', () => {
    const group: ProjectGroup = {
      id: 'group-1',
      name: 'Platform',
      parentPath: '/platform',
      parentGroupId: null,
      createdFrom: 'folder-scan',
      tabOrder: 0,
      isCollapsed: false,
      color: null,
      createdAt: 1,
      updatedAt: 1
    }
    const groupedRepo: Repo = { ...repo, projectGroupId: group.id }

    const rows = buildRows(
      'repo',
      [],
      new Map([[groupedRepo.id, groupedRepo]]),
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      {},
      new Map(),
      false,
      undefined,
      [group],
      new Set([groupedRepo.id])
    )

    expect(rows[0]).toMatchObject({
      type: 'header',
      key: 'project-group:group-1',
      count: 1
    })
  })

  it('does not resurrect filtered repos as empty Project Group headers', () => {
    const group: ProjectGroup = {
      id: 'group-1',
      name: 'Platform',
      parentPath: '/platform',
      parentGroupId: null,
      createdFrom: 'folder-scan',
      tabOrder: 0,
      isCollapsed: false,
      color: null,
      createdAt: 1,
      updatedAt: 1
    }
    const groupedRepo: Repo = { ...repo, projectGroupId: group.id }

    const rows = buildRows(
      'repo',
      [],
      new Map([[groupedRepo.id, groupedRepo]]),
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      {},
      new Map(),
      false,
      undefined,
      [group]
    )

    expect(rows.filter((row) => row.type === 'header').map((row) => row.key)).toEqual([
      'project-group:group-1'
    ])
    expect(rows[0]).toMatchObject({ count: 0 })
  })

  it('renders ungrouped repos as top-level repo rows when Project Groups exist', () => {
    const group: ProjectGroup = {
      id: 'group-1',
      name: 'Platform',
      parentPath: '/platform',
      parentGroupId: null,
      createdFrom: 'folder-scan',
      tabOrder: 0,
      isCollapsed: false,
      color: null,
      createdAt: 1,
      updatedAt: 1
    }

    const rows = buildRows(
      'repo',
      [worktree],
      repoMap,
      null,
      new Set(),
      new Map([[repo.id, 0]]),
      undefined,
      'manual',
      {},
      new Map([[worktree.id, worktree]]),
      false,
      undefined,
      [group]
    )

    expect(rows.filter((row) => row.type === 'header').map((row) => row.key)).toEqual([
      'project-group:group-1',
      'repo:repo-1'
    ])
  })

  it('orders repos inside a Project Group by projectGroupOrder in manual mode', () => {
    const group: ProjectGroup = {
      id: 'group-1',
      name: 'Platform',
      parentPath: '/platform',
      parentGroupId: null,
      createdFrom: 'folder-scan',
      tabOrder: 0,
      isCollapsed: false,
      color: null,
      createdAt: 1,
      updatedAt: 1
    }
    const repoA: Repo = {
      ...repo,
      id: 'repo-a',
      displayName: 'alpha',
      projectGroupId: group.id,
      projectGroupOrder: 1
    }
    const repoB: Repo = {
      ...repo,
      id: 'repo-b',
      displayName: 'beta',
      projectGroupId: group.id,
      projectGroupOrder: 0
    }
    const worktreeA: Worktree = { ...worktree, id: 'wt-a', repoId: repoA.id }
    const worktreeB: Worktree = { ...worktree, id: 'wt-b', repoId: repoB.id }
    const groupedMap = new Map([
      [repoA.id, repoA],
      [repoB.id, repoB]
    ])
    const repoOrder = new Map([
      [repoA.id, 0],
      [repoB.id, 1]
    ])

    const rows = buildRows(
      'repo',
      [worktreeA, worktreeB],
      groupedMap,
      null,
      new Set(),
      repoOrder,
      undefined,
      'manual',
      undefined,
      undefined,
      false,
      undefined,
      [group]
    )

    expect(rows.filter((row) => row.type === 'header').map((row) => row.key)).toEqual([
      'project-group:group-1',
      'repo:repo-b',
      'repo:repo-a'
    ])
  })

  it('renders nested Project Groups before repos assigned to their leaf group', () => {
    const rootGroup: ProjectGroup = {
      id: 'group-root',
      name: 'Services',
      parentPath: '/monorepo',
      parentGroupId: null,
      createdFrom: 'folder-scan',
      tabOrder: 0,
      isCollapsed: false,
      color: null,
      createdAt: 1,
      updatedAt: 1
    }
    const childGroup: ProjectGroup = {
      ...rootGroup,
      id: 'group-payments',
      name: 'payments',
      parentPath: '/monorepo/services/payments',
      parentGroupId: rootGroup.id,
      tabOrder: 1
    }
    const groupedRepo: Repo = {
      ...repo,
      id: 'repo-payments-api',
      displayName: 'api',
      projectGroupId: childGroup.id,
      projectGroupOrder: 0
    }
    const groupedWorktree: Worktree = {
      ...worktree,
      id: 'wt-payments-api',
      repoId: groupedRepo.id
    }

    const rows = buildRows(
      'repo',
      [groupedWorktree],
      new Map([[groupedRepo.id, groupedRepo]]),
      null,
      new Set(),
      new Map([[groupedRepo.id, 0]]),
      undefined,
      'manual',
      undefined,
      undefined,
      false,
      undefined,
      [rootGroup, childGroup]
    )

    expect(rows.filter((row) => row.type === 'header').map((row) => row.key)).toEqual([
      'project-group:group-root',
      'project-group:group-payments',
      'repo:repo-payments-api'
    ])
    expect(rows.filter((row) => row.type === 'header').map((row) => row.projectGroupDepth)).toEqual(
      [0, 1, 2]
    )
    expect(rows[0]).toMatchObject({ count: 1 })
  })

  it('renders imported repos under nested Project Groups before worktree rows load', () => {
    const rootGroup: ProjectGroup = {
      id: 'group-root',
      name: 'Root',
      parentPath: '/monorepo',
      parentGroupId: null,
      createdFrom: 'folder-scan',
      tabOrder: 0,
      isCollapsed: false,
      color: null,
      createdAt: 1,
      updatedAt: 1
    }
    const platformGroup: ProjectGroup = {
      ...rootGroup,
      id: 'group-platform',
      name: 'Platform',
      parentGroupId: rootGroup.id,
      tabOrder: 1
    }
    const servicesGroup: ProjectGroup = {
      ...rootGroup,
      id: 'group-services',
      name: 'Services',
      parentGroupId: platformGroup.id,
      tabOrder: 2
    }
    const serviceA: Repo = {
      ...repo,
      id: 'repo-service-a',
      displayName: 'service-a',
      projectGroupId: servicesGroup.id,
      projectGroupOrder: 0
    }
    const serviceB: Repo = {
      ...repo,
      id: 'repo-service-b',
      displayName: 'service-b',
      projectGroupId: servicesGroup.id,
      projectGroupOrder: 1
    }

    const rows = buildRows(
      'repo',
      [],
      new Map([
        [serviceA.id, serviceA],
        [serviceB.id, serviceB]
      ]),
      null,
      new Set(),
      new Map([
        [serviceA.id, 0],
        [serviceB.id, 1]
      ]),
      undefined,
      'manual',
      undefined,
      undefined,
      false,
      undefined,
      [rootGroup, platformGroup, servicesGroup],
      new Set([serviceA.id, serviceB.id])
    )

    expect(rows.filter((row) => row.type === 'header').map((row) => row.key)).toEqual([
      'project-group:group-root',
      'project-group:group-platform',
      'project-group:group-services',
      'repo:repo-service-a',
      'repo:repo-service-b'
    ])
    expect(rows.filter((row) => row.type === 'header').map((row) => row.count)).toEqual([
      2, 2, 2, 0, 0
    ])
  })

  it('returns both parent Project Group and repo keys for grouped repo reveals', () => {
    const groupedRepo: Repo = { ...repo, projectGroupId: 'group-1' }

    expect(
      getGroupKeysForWorktree('repo', worktree, new Map([[groupedRepo.id, groupedRepo]]), null)
    ).toEqual(['project-group:group-1', 'repo:repo-1'])
  })

  it('returns only the repo key for ungrouped repo reveals', () => {
    expect(getGroupKeysForWorktree('repo', worktree, repoMap, null)).toEqual(['repo:repo-1'])
  })
})

describe('buildRows workspace lineage nesting', () => {
  const parent: Worktree = {
    ...worktree,
    id: 'wt-parent',
    instanceId: 'parent-instance',
    displayName: 'coordinator'
  }
  const child: Worktree = {
    ...worktree,
    id: 'wt-child',
    instanceId: 'child-instance',
    displayName: 'worker'
  }
  const grandchild: Worktree = {
    ...worktree,
    id: 'wt-grandchild',
    instanceId: 'grandchild-instance',
    displayName: 'nested-worker'
  }
  const lineage: WorktreeLineage = {
    worktreeId: child.id,
    worktreeInstanceId: 'child-instance',
    parentWorktreeId: parent.id,
    parentWorktreeInstanceId: 'parent-instance',
    origin: 'cli',
    capture: { source: 'terminal-context', confidence: 'inferred' },
    createdAt: 1
  }
  const grandchildLineage: WorktreeLineage = {
    worktreeId: grandchild.id,
    worktreeInstanceId: 'grandchild-instance',
    parentWorktreeId: child.id,
    parentWorktreeInstanceId: 'child-instance',
    origin: 'cli',
    capture: { source: 'terminal-context', confidence: 'inferred' },
    createdAt: 1
  }

  it('keeps lineage flat when nesting is off', () => {
    const rows = buildRows(
      'none',
      [child, parent],
      repoMap,
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      { [child.id]: lineage },
      new Map([
        [parent.id, parent],
        [child.id, child]
      ])
    )

    const items = rows.filter((row) => row.type === 'item')
    expect(items[0]).toMatchObject({ type: 'item', worktree: { id: child.id } })
    expect(items[0]).not.toHaveProperty('parentLabel')
    expect(items[1]).toMatchObject({
      type: 'item',
      worktree: { id: parent.id }
    })
  })

  it('places children directly under their parent when nesting is on', () => {
    const rows = buildRows(
      'none',
      [child, parent],
      repoMap,
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      { [child.id]: lineage },
      new Map([
        [parent.id, parent],
        [child.id, child]
      ]),
      true
    )

    const items = rows.filter((row) => row.type === 'item')
    expect(items[0]).toMatchObject({ type: 'item', worktree: { id: parent.id } })
    expect(items[1]).toMatchObject({
      type: 'item',
      worktree: { id: child.id },
      depth: 1
    })
  })

  it('supports nested lineage chains beyond one level', () => {
    const rows = buildRows(
      'none',
      [grandchild, child, parent],
      repoMap,
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      { [child.id]: lineage, [grandchild.id]: grandchildLineage },
      new Map([
        [parent.id, parent],
        [child.id, child],
        [grandchild.id, grandchild]
      ]),
      true
    )

    const items = rows.filter((row) => row.type === 'item')
    expect(items.map((row) => row.worktree.id)).toEqual([parent.id, child.id, grandchild.id])
    expect(items[0]).toMatchObject({
      type: 'item',
      depth: 0,
      lineageChildCount: 1,
      lineageCollapsed: false
    })
    expect(items[1]).toMatchObject({
      type: 'item',
      worktree: { id: child.id },
      depth: 1,
      lineageChildCount: 1
    })
    expect(items[2]).toMatchObject({
      type: 'item',
      worktree: { id: grandchild.id },
      depth: 2,
      lineageChildCount: 0
    })
  })

  it('collapses descendants under lineage parents', () => {
    const rows = buildRows(
      'none',
      [grandchild, child, parent],
      repoMap,
      null,
      new Set([getLineageGroupKey(parent.id)]),
      undefined,
      undefined,
      undefined,
      { [child.id]: lineage, [grandchild.id]: grandchildLineage },
      new Map([
        [parent.id, parent],
        [child.id, child],
        [grandchild.id, grandchild]
      ]),
      true
    )

    const items = rows.filter((row) => row.type === 'item')
    expect(items).toHaveLength(1)
    expect(items[0]).toMatchObject({
      type: 'item',
      worktree: { id: parent.id },
      lineageChildCount: 1,
      lineageCollapsed: true
    })
  })

  it('does not create a parent group for stale instance links', () => {
    const staleLineage = { ...lineage, parentWorktreeInstanceId: 'old-parent-instance' }
    const rows = buildRows(
      'none',
      [child],
      repoMap,
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      { [child.id]: staleLineage },
      new Map([
        [parent.id, parent],
        [child.id, child]
      ]),
      true
    )

    const items = rows.filter((row) => row.type === 'item')
    expect(items[0]).toMatchObject({
      type: 'item',
      worktree: { id: child.id },
      depth: 0
    })
  })

  it('marks stale instance links as missing for shared context-menu validation', () => {
    const staleLineage = { ...lineage, parentWorktreeInstanceId: 'old-parent-instance' }
    const info = getLineageRenderInfo(
      child,
      { [child.id]: staleLineage },
      new Map([
        [parent.id, parent],
        [child.id, child]
      ])
    )

    expect(info).toMatchObject({ state: 'missing' })
  })

  it('keeps pinned children in Pinned without a parent badge', () => {
    const pinnedChild = { ...child, isPinned: true }
    const rows = buildRows(
      'none',
      [parent, pinnedChild],
      repoMap,
      null,
      new Set(),
      undefined,
      undefined,
      undefined,
      { [child.id]: lineage },
      new Map([
        [parent.id, parent],
        [child.id, pinnedChild]
      ]),
      true
    )

    expect(rows[0]).toMatchObject({ type: 'header', key: 'pinned' })
    expect(rows[1]).toMatchObject({
      type: 'item',
      worktree: { id: child.id }
    })
    expect(rows[1]).not.toHaveProperty('parentLabel')
  })
})

describe('groupRowsByHost', () => {
  const hostForKey = (key: string): SidebarHost => ({
    key,
    kind: key === 'local' ? 'local' : 'ssh',
    label: key === 'local' ? 'studio' : 'forge',
    status: 'reachable'
  })

  // A repo header carries `.repo` (with optional connectionId) and a count.
  const repoHeader = (id: string, connectionId: string | null, count: number): Row =>
    ({ type: 'header', key: `repo:${id}`, label: id, count, tone: '', repo: { connectionId } } as unknown as Row)
  const item = (id: string, repoConnectionId: string | null): Row =>
    ({ type: 'item', worktree: { id }, repo: { connectionId: repoConnectionId }, depth: 0, lineageTrail: [], isLastLineageChild: false, lineageChildCount: 0 } as unknown as Row)

  it('inserts a host header above each host group, local first', () => {
    const rows: Row[] = [
      repoHeader('remote-repo', 'conn-1', 1),
      item('w1', 'conn-1'),
      repoHeader('local-repo', null, 1),
      item('w2', null)
    ]
    const out = groupRowsByHost(rows, hostForKey, new Set())
    expect(out.map((r) => r.type)).toEqual([
      'host-header', 'header', 'item', // local host first
      'host-header', 'header', 'item' // then ssh host
    ])
    expect((out[0] as { host: SidebarHost }).host.key).toBe('local')
    expect((out[3] as { host: SidebarHost }).host.key).toBe('ssh:conn-1')
  })

  it('sums repo header counts into the host count', () => {
    const rows: Row[] = [repoHeader('a', null, 2), item('w1', null), repoHeader('b', null, 3), item('w2', null)]
    const out = groupRowsByHost(rows, hostForKey, new Set())
    expect((out[0] as { count: number }).count).toBe(5)
  })

  it('hides a host group body when its host header is collapsed', () => {
    const rows: Row[] = [repoHeader('a', null, 1), item('w1', null)]
    const collapsed = new Set([getHostHeaderKey('local')])
    const out = groupRowsByHost(rows, hostForKey, collapsed)
    expect(out.map((r) => r.type)).toEqual(['host-header'])
  })

  it('keeps a leading Pinned section above all hosts', () => {
    const rows: Row[] = [
      { type: 'header', key: PINNED_GROUP_KEY, label: 'Pinned', count: 1, tone: '' } as Row,
      item('p1', null),
      repoHeader('a', null, 1),
      item('w1', null)
    ]
    const out = groupRowsByHost(rows, hostForKey, new Set())
    expect(out[0].type).toBe('header')
    expect((out[0] as { key: string }).key).toBe(PINNED_GROUP_KEY)
    expect(out[1].type).toBe('item')
    expect(out[2].type).toBe('host-header')
  })
})

describe('WorktreeList header styles', () => {
  it('does not title-case workspace group labels', () => {
    const source = readFileSync(
      fileURLToPath(new URL('./WorktreeList.tsx', import.meta.url)),
      'utf8'
    )

    expect(source).not.toContain('leading-none capitalize')
  })

  it('shows a pointer cursor over the disclosure chevron path', () => {
    const source = readFileSync(
      fileURLToPath(new URL('./WorktreeList.tsx', import.meta.url)),
      'utf8'
    )

    expect(source).toContain('[&_path]:cursor-pointer')
  })

  it('renders the resolved project color mark in repo headers', () => {
    const source = readFileSync(
      fileURLToPath(new URL('./WorktreeList.tsx', import.meta.url)),
      'utf8'
    )

    expect(source).toContain('resolveProjectGroupHeaderColor({')
    expect(source).toContain('headerKey: row.key')
    expect(source).toMatch(
      /row\.repo\s*\?\s*\(\s*<RepoBadgeMark\s+color=\{repoHeaderColor\}\s+className="size-2 shrink-0 rounded-\[2px\]"/
    )
  })

  it('keeps project color independent from active and selected header styling', () => {
    const source = readFileSync(
      fileURLToPath(new URL('./WorktreeList.tsx', import.meta.url)),
      'utf8'
    )

    // The project color remains confined to an unconditional decorative mark,
    // leaving theme-owned row backgrounds and foreground contrast intact.
    expect(source).toMatch(
      /row\.repo\s*\?\s*\(\s*<RepoBadgeMark\s+color=\{repoHeaderColor\}\s+className="size-2 shrink-0 rounded-\[2px\]"/
    )
    expect(source).toMatch(
      /projectHubRepoId === row\.repo\.id\s*&&\s*'rounded-md bg-sidebar-accent'/
    )
    expect(source).toMatch(
      /isProjectSelected\s*&&\s*'rounded-md bg-sidebar-accent ring-1 ring-sidebar-ring\/35'/
    )
    expect(source).toContain(
      'min-w-0 truncate text-[13px] font-semibold leading-none'
    )
  })

  it('uses the sidebar foreground token for readable light and dark theme content', () => {
    const source = readFileSync(
      fileURLToPath(new URL('./WorktreeList.tsx', import.meta.url)),
      'utf8'
    )

    // Project color is an identity cue, not a text color. The header's label and
    // inherited icons therefore follow the foreground paired with each theme's
    // sidebar surface, while the configured color remains on the mark/glyph.
    expect(source).toContain(
      'pr-1 text-left text-sidebar-foreground transition-all'
    )
    expect(source).toMatch(
      /<RepoIconGlyph\s+repoIcon=\{row\.repo\.repoIcon\}\s+color=\{repoHeaderColor\}/
    )
    expect(source).toMatch(
      /<RepoBadgeMark\s+color=\{repoHeaderColor\}/
    )
  })
})

describe('pickWorktreeForHost', () => {
  const localMain: Worktree = { ...worktree, id: 'local-main', isMainWorktree: true }
  const localChild: Worktree = { ...worktree, id: 'local-child', isMainWorktree: false }
  const sshRepo: Repo = { ...repo, id: 'repo-ssh', connectionId: 'conn-1' }
  const sshChild: Worktree = {
    ...worktree,
    id: 'ssh-child',
    repoId: sshRepo.id,
    isMainWorktree: false
  }
  const sshMain: Worktree = {
    ...worktree,
    id: 'ssh-main',
    repoId: sshRepo.id,
    isMainWorktree: true
  }
  const map = new Map<string, Repo>([
    [repo.id, repo],
    [sshRepo.id, sshRepo]
  ])
  const all = [localChild, localMain, sshChild, sshMain]

  it('returns null when the host has no worktrees', () => {
    expect(pickWorktreeForHost('ssh:nope', all, map, null)).toBeNull()
  })

  it('prefers the active worktree when it lives on the host', () => {
    expect(pickWorktreeForHost(LOCAL_HOST_KEY, all, map, 'local-child')?.id).toBe('local-child')
  })

  it('falls back to the main worktree when the active one is on another host', () => {
    // active worktree is on the ssh host, so the local pick ignores it
    expect(pickWorktreeForHost(LOCAL_HOST_KEY, all, map, 'ssh-main')?.id).toBe('local-main')
  })

  it('buckets ssh worktrees under their connection host key', () => {
    expect(pickWorktreeForHost('ssh:conn-1', all, map, null)?.id).toBe('ssh-main')
  })

  it('falls back to the first worktree when none is main', () => {
    expect(pickWorktreeForHost(LOCAL_HOST_KEY, [localChild], map, null)?.id).toBe('local-child')
  })
})

describe('sidebarHostStatus', () => {
  it('maps connected to reachable', () => {
    expect(sidebarHostStatus('connected')).toBe('reachable')
  })

  it('maps every in-flight transport state to connecting', () => {
    expect(sidebarHostStatus('connecting')).toBe('connecting')
    expect(sidebarHostStatus('deploying-relay')).toBe('connecting')
    expect(sidebarHostStatus('reconnecting')).toBe('connecting')
  })

  it('maps every not-connected transport state to down — an outage must not render as unknown', () => {
    expect(sidebarHostStatus('disconnected')).toBe('down')
    expect(sidebarHostStatus('auth-failed')).toBe('down')
    expect(sidebarHostStatus('reconnection-failed')).toBe('down')
    expect(sidebarHostStatus('error')).toBe('down')
  })

  it('reserves unknown for hosts with no transport record', () => {
    expect(sidebarHostStatus(undefined)).toBe('unknown')
  })
})
