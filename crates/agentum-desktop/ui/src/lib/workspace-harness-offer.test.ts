// Spec 015 — runner pins: the async offer shell around the pure detect model.
// Real store (seeded like open-created-workspace.test.ts), mocked runtime
// clients; no network, no fs.
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { Worktree } from '../../../shared/types'
import type { FsFileEntry } from '@/runtime/server-fs-client'
import { useAppStore } from '@/store'
import { fsListEntries } from '@/runtime/server-fs-client'
import { maybeOfferWorkspaceHarnessRun } from './workspace-harness-offer'

vi.mock('@/runtime/server-fs-client', () => ({
  fsListEntries: vi.fn()
}))

vi.mock('@/runtime/harness-client', () => ({
  listHarnesses: vi.fn(() => Promise.resolve([])),
  startHarness: vi.fn(),
  runHarness: vi.fn(),
  subscribeHarnessRunErrors: vi.fn(() => Promise.resolve({ close: () => {} }))
}))

const initialAppStoreState = useAppStore.getState()

afterEach(() => {
  vi.clearAllMocks()
  useAppStore.setState(initialAppStoreState, true)
})

const WT_ID = 'repo-1::/workspace/feature'

function makeWorktree(): Worktree {
  return {
    id: WT_ID,
    repoId: 'repo-1',
    path: '/workspace/feature',
    head: 'abc123',
    branch: 'refs/heads/feature',
    isBare: false,
    isMainWorktree: false,
    displayName: 'feature',
    comment: '',
    linkedIssue: null,
    linkedPR: null,
    linkedLinearIssue: null,
    isArchived: false,
    isUnread: false,
    isPinned: false,
    sortOrder: 0,
    lastActivityAt: 0,
    createdWithAgent: 'codex'
  }
}

function seedStore(opts?: { connectionId?: string | null }): void {
  useAppStore.setState({
    repos: [
      {
        id: 'repo-1',
        path: '/workspace/repo',
        displayName: 'repo',
        badgeColor: '#000000',
        addedAt: 0,
        ...(opts?.connectionId !== undefined ? { connectionId: opts.connectionId } : {})
      }
    ],
    worktreesByRepo: { 'repo-1': [makeWorktree()] }
  })
}

function specFileEntry(): FsFileEntry {
  return { name: 'feature_list.json', path: '/x/feature_list.json', kind: 'file' }
}

function listing(entries: FsFileEntry[]): { path: string; parent: null; entries: FsFileEntry[] } {
  return { path: '/x', parent: null, entries }
}

function offers(): Record<string, unknown> {
  return useAppStore.getState().harnessOfferByWorktreeId
}

describe('maybeOfferWorkspaceHarnessRun', () => {
  it('remote repo (connectionId set): zero fs calls, no offer (D5)', async () => {
    seedStore({ connectionId: 'c1' })
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: false })
    expect(fsListEntries).not.toHaveBeenCalled()
    expect(offers()).toEqual({})
  })

  it('gated run: zero fs calls, no offer (D6)', async () => {
    seedStore()
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: true })
    expect(fsListEntries).not.toHaveBeenCalled()
    expect(offers()).toEqual({})
  })

  it('purges a stale offer for the same worktree id at runner start', async () => {
    seedStore()
    useAppStore.getState().setWorkspaceHarnessOffer({
      worktreeId: WT_ID,
      workdir: '/workspace/feature',
      harnessDir: '.agentum-harness'
    })
    // Gated recreation at the same path: the pre-close offer must not leak.
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: true })
    expect(offers()).toEqual({})
  })

  it('canonical dir missing, legacy has the file: offer set with .harness', async () => {
    seedStore()
    vi.mocked(fsListEntries)
      .mockRejectedValueOnce(new Error('path error: missing'))
      .mockResolvedValueOnce(listing([specFileEntry()]))
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: false })
    expect(fsListEntries).toHaveBeenNthCalledWith(1, '/workspace/feature/.agentum-harness', {
      hidden: true
    })
    expect(fsListEntries).toHaveBeenNthCalledWith(2, '/workspace/feature/.harness', {
      hidden: true
    })
    expect(offers()).toEqual({
      [WT_ID]: {
        worktreeId: WT_ID,
        workdir: '/workspace/feature',
        harnessDir: '.harness'
      }
    })
  })

  it('canonical dir has the file: exactly ONE fs call, offer set', async () => {
    seedStore()
    vi.mocked(fsListEntries).mockResolvedValueOnce(listing([specFileEntry()]))
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: false })
    expect(fsListEntries).toHaveBeenCalledTimes(1)
    expect(offers()).toEqual({
      [WT_ID]: {
        worktreeId: WT_ID,
        workdir: '/workspace/feature',
        harnessDir: '.agentum-harness'
      }
    })
  })

  it('nothing found: no offer, no harness-client call (AC 6)', async () => {
    seedStore()
    const { listHarnesses } = await import('@/runtime/harness-client')
    vi.mocked(fsListEntries)
      .mockRejectedValueOnce(new Error('path error: missing'))
      .mockRejectedValueOnce(new Error('path error: missing'))
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: false })
    expect(fsListEntries).toHaveBeenCalledTimes(2)
    expect(listHarnesses).not.toHaveBeenCalled()
    expect(offers()).toEqual({})
  })

  it('canonical dir present WITHOUT the file: legacy never fetched, no offer', async () => {
    seedStore()
    vi.mocked(fsListEntries).mockResolvedValueOnce(
      listing([{ name: 'AGENTS.md', path: '/x/AGENTS.md', kind: 'file' }])
    )
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: false })
    expect(fsListEntries).toHaveBeenCalledTimes(1)
    expect(offers()).toEqual({})
  })

  it('worktree closed while detection was in flight: signal dropped', async () => {
    seedStore()
    vi.mocked(fsListEntries).mockImplementationOnce(async () => {
      // The workspace disappears between the fs fetch and the slice write.
      useAppStore.setState({ worktreesByRepo: {} })
      return listing([specFileEntry()])
    })
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: false })
    expect(offers()).toEqual({})
  })

  it('unknown worktree id: fail closed, zero fs calls', async () => {
    seedStore()
    await maybeOfferWorkspaceHarnessRun({ worktreeId: 'repo-1::/nope', gatedRun: false })
    expect(fsListEntries).not.toHaveBeenCalled()
    expect(offers()).toEqual({})
  })
})
