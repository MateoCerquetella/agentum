// Spec 015 — runner pins: the async offer shell around the pure detect model.
// Real store (seeded like open-created-workspace.test.ts), mocked runtime
// clients; no network, no fs.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { toast } from 'sonner'
import type { Worktree } from '../../../shared/types'
import type { FsFileEntry } from '@/runtime/server-fs-client'
import { useAppStore } from '@/store'
import { fsListEntries } from '@/runtime/server-fs-client'
import {
  listHarnesses,
  runHarness,
  startHarness,
  subscribeHarnessRunErrors
} from '@/runtime/harness-client'
import { acceptHarnessOffer, maybeOfferWorkspaceHarnessRun } from './workspace-harness-offer'

vi.mock('@/runtime/server-fs-client', () => ({
  fsListEntries: vi.fn()
}))

vi.mock('@/runtime/harness-client', () => ({
  listHarnesses: vi.fn(() => Promise.resolve([])),
  startHarness: vi.fn(),
  runHarness: vi.fn(),
  subscribeHarnessRunErrors: vi.fn(() => Promise.resolve({ close: () => {} }))
}))

vi.mock('sonner', () => ({
  toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() }
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

function starting(): Record<string, unknown> {
  return useAppStore.getState().gatedRunStartingByWorktreeId
}

describe('maybeOfferWorkspaceHarnessRun', () => {
  it('remote repo (connectionId set): zero fs calls, no offer (D5)', async () => {
    seedStore({ connectionId: 'c1' })
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: false })
    expect(fsListEntries).not.toHaveBeenCalled()
    expect(offers()).toEqual({})
  })

  it('gated run: zero fs calls, no offer (D6) — but the starting slice is set (spec 023 AC 1)', async () => {
    seedStore()
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: true })
    expect(fsListEntries).not.toHaveBeenCalled()
    expect(offers()).toEqual({})
    // The owned gated run surfaces as "starting" in the workspace view.
    expect(starting()).toEqual({
      [WT_ID]: { worktreeId: WT_ID, workdir: '/workspace/feature' }
    })
  })

  it('gated run on an unknown worktree fails closed (no starting slice)', async () => {
    seedStore()
    await maybeOfferWorkspaceHarnessRun({ worktreeId: 'repo-1::/gone', gatedRun: true })
    expect(starting()).toEqual({})
  })

  it('purges a stale STARTING slice for the same worktree id at runner start', async () => {
    seedStore()
    useAppStore.getState().setGatedRunStarting({ worktreeId: WT_ID, workdir: '/workspace/feature' })
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: false })
    expect(starting()).toEqual({})
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
    vi.mocked(fsListEntries)
      .mockRejectedValueOnce(new Error('path error: missing'))
      .mockRejectedValueOnce(new Error('path error: missing'))
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: false })
    expect(fsListEntries).toHaveBeenCalledTimes(2)
    expect(listHarnesses).not.toHaveBeenCalled()
    expect(offers()).toEqual({})
  })

  it('workdir already registered with the engine: no offer (AC 5)', async () => {
    seedStore()
    vi.mocked(fsListEntries).mockResolvedValueOnce(listing([specFileEntry()]))
    vi.mocked(listHarnesses).mockResolvedValueOnce([
      // Trailing-slash spelling: normalization must still dedupe it.
      { workdir: '/workspace/feature/' }
    ] as unknown as Awaited<ReturnType<typeof listHarnesses>>)
    await maybeOfferWorkspaceHarnessRun({ worktreeId: WT_ID, gatedRun: false })
    expect(listHarnesses).toHaveBeenCalledTimes(1)
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

// Spec 015 f3 — the accept flow: exactly register + run (the gate is sacred),
// success clears the offer, failure keeps it retryable with the server's
// detail in the toast.
describe('acceptHarnessOffer', () => {
  const OFFER = {
    worktreeId: WT_ID,
    workdir: '/workspace/feature',
    harnessDir: '.agentum-harness'
  } as const

  function seedOffer(): void {
    seedStore()
    useAppStore.getState().setWorkspaceHarnessOffer(OFFER)
  }

  it('happy path: register then run in order, success toast, slice cleared, error stream armed', async () => {
    seedOffer()
    vi.mocked(startHarness).mockResolvedValueOnce({ harness_id: 'h-1' })
    vi.mocked(runHarness).mockResolvedValueOnce(undefined)

    await acceptHarnessOffer(OFFER)

    expect(startHarness).toHaveBeenCalledWith('/workspace/feature')
    expect(runHarness).toHaveBeenCalledWith('h-1')
    expect(vi.mocked(startHarness).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(runHarness).mock.invocationCallOrder[0]!
    )
    expect(toast.success).toHaveBeenCalledWith('Harness run started')
    expect(offers()).toEqual({})
    // Early drive-phase failures (red init.sh, spawn errors) surface via the
    // bounded, id-scoped subscription.
    expect(subscribeHarnessRunErrors).toHaveBeenCalledWith('h-1', expect.any(Function))
  })

  it('startHarness failure: toast carries the server detail, run never fires, slice KEPT', async () => {
    seedOffer()
    vi.mocked(startHarness).mockRejectedValueOnce(
      new Error('harness 400 on /api/harness — workdir does not exist')
    )

    await acceptHarnessOffer(OFFER)

    expect(runHarness).not.toHaveBeenCalled()
    expect(toast.error).toHaveBeenCalledWith(expect.stringContaining('workdir does not exist'))
    expect(offers()).toEqual({ [WT_ID]: OFFER })
    expect(subscribeHarnessRunErrors).not.toHaveBeenCalled()
  })

  it('runHarness failure: toast carries the detail, slice KEPT', async () => {
    seedOffer()
    vi.mocked(startHarness).mockResolvedValueOnce({ harness_id: 'h-1' })
    vi.mocked(runHarness).mockRejectedValueOnce(
      new Error('harness 409 on /api/harness/h-1/run — already running')
    )

    await acceptHarnessOffer(OFFER)

    expect(toast.error).toHaveBeenCalledWith(expect.stringContaining('already running'))
    expect(offers()).toEqual({ [WT_ID]: OFFER })
    expect(subscribeHarnessRunErrors).not.toHaveBeenCalled()
  })

  it('dismiss clears the slice with ZERO harness-client calls (AC 4)', () => {
    seedOffer()

    useAppStore.getState().clearWorkspaceHarnessOffer(WT_ID)

    expect(offers()).toEqual({})
    expect(startHarness).not.toHaveBeenCalled()
    expect(runHarness).not.toHaveBeenCalled()
    expect(listHarnesses).not.toHaveBeenCalled()
    expect(subscribeHarnessRunErrors).not.toHaveBeenCalled()
  })
})
