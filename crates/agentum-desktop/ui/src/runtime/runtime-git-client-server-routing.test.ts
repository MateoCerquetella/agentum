// Verifies the newly server-backed git ops (Slice A: checkIgnored, history,
// fastForward, branchDiff, commitDiff, remoteFileUrl) dispatch to the embedded
// server adapter when server-git is enabled and the target is a local workspace.
// The adapter module is mocked so the test targets the routing decision, not HTTP.
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('./server-git-adapter', () => ({
  // Only the six this suite exercises need behaviour; the rest exist so the
  // runtime client's imports resolve under the mock.
  getServerGitCheckIgnored: vi.fn(),
  serverGitFastForward: vi.fn(),
  getServerGitRemoteFileUrl: vi.fn(),
  getServerGitCommitDiff: vi.fn(),
  getServerGitBranchDiff: vi.fn(),
  getServerGitHistory: vi.fn(),
  getServerGitStatus: vi.fn(),
  getServerGitConflictOperation: vi.fn(),
  getServerGitUpstreamStatus: vi.fn(),
  serverGitStage: vi.fn(),
  getServerGitBranchCompare: vi.fn(),
  getServerGitCommitCompare: vi.fn(),
  serverGitFetch: vi.fn(),
  serverGitPull: vi.fn(),
  serverGitCommit: vi.fn(),
  serverGitDiscard: vi.fn(),
  serverGitPush: vi.fn(),
  serverGitRebase: vi.fn(),
  serverGitAbortMerge: vi.fn(),
  serverGitAbortRebase: vi.fn(),
  getServerGitDiff: vi.fn()
}))

import {
  getServerGitCheckIgnored,
  serverGitFastForward,
  getServerGitRemoteFileUrl,
  getServerGitCommitDiff,
  getServerGitBranchDiff,
  getServerGitHistory
} from './server-git-adapter'
import {
  fastForwardRuntimeGit,
  getRuntimeGitBranchDiff,
  getRuntimeGitCommitDiff,
  getRuntimeGitHistory,
  getRuntimeGitIgnoredPaths,
  getRuntimeGitRemoteFileUrl
} from './runtime-git-client'

const LOCAL = {
  settings: { activeRuntimeEnvironmentId: null },
  worktreeId: 'wt-1',
  worktreePath: '/repo'
} as const

beforeEach(() => {
  vi.clearAllMocks()
  // Server-git defaults ON; ensure the flag isn't disabled by a prior test.
  globalThis.localStorage?.removeItem('agentum.serverTerminals')
})

describe('runtime git client — server routing (local workspace)', () => {
  it('routes checkIgnored to the server adapter', async () => {
    vi.mocked(getServerGitCheckIgnored).mockResolvedValue(['dist/bundle.js'])
    const result = await getRuntimeGitIgnoredPaths(LOCAL, ['dist/bundle.js', 'src/index.ts'])
    expect(getServerGitCheckIgnored).toHaveBeenCalledWith('/repo', [
      'dist/bundle.js',
      'src/index.ts'
    ])
    expect(result).toEqual(['dist/bundle.js'])
  })

  it('routes history to the server adapter', async () => {
    const history = {
      items: [],
      hasIncomingChanges: false,
      hasOutgoingChanges: false,
      hasMore: false,
      limit: 25
    }
    vi.mocked(getServerGitHistory).mockResolvedValue(history as never)
    const result = await getRuntimeGitHistory(LOCAL, { limit: 25, baseRef: 'origin/main' })
    expect(getServerGitHistory).toHaveBeenCalledWith('/repo', { limit: 25, baseRef: 'origin/main' })
    expect(result).toBe(history)
  })

  it('routes fastForward to the server adapter (no explicit push target)', async () => {
    vi.mocked(serverGitFastForward).mockResolvedValue()
    await fastForwardRuntimeGit(LOCAL)
    expect(serverGitFastForward).toHaveBeenCalledWith('/repo')
  })

  it('routes commitDiff to the server adapter', async () => {
    const diff = {
      kind: 'text' as const,
      originalContent: 'a',
      modifiedContent: 'b',
      originalIsBinary: false as const,
      modifiedIsBinary: false as const
    }
    vi.mocked(getServerGitCommitDiff).mockResolvedValue(diff)
    const args = { commitOid: 'abc', parentOid: 'def', filePath: 'src/a.ts' }
    const result = await getRuntimeGitCommitDiff(LOCAL, args)
    expect(getServerGitCommitDiff).toHaveBeenCalledWith('/repo', args)
    expect(result).toBe(diff)
  })

  it('routes branchDiff to the server adapter', async () => {
    const diff = {
      kind: 'text' as const,
      originalContent: '',
      modifiedContent: 'x',
      originalIsBinary: false as const,
      modifiedIsBinary: false as const
    }
    vi.mocked(getServerGitBranchDiff).mockResolvedValue(diff)
    const args = {
      compare: { baseRef: 'main', baseOid: 'b1', headOid: 'h1', mergeBase: 'm1' },
      filePath: 'src/a.ts'
    }
    const result = await getRuntimeGitBranchDiff(LOCAL, args)
    expect(getServerGitBranchDiff).toHaveBeenCalledWith('/repo', args)
    expect(result).toBe(diff)
  })

  it('routes remoteFileUrl to the server adapter', async () => {
    vi.mocked(getServerGitRemoteFileUrl).mockResolvedValue('https://github.com/o/r/blob/main/a.ts#L3')
    const result = await getRuntimeGitRemoteFileUrl(LOCAL, { relativePath: 'a.ts', line: 3 })
    expect(getServerGitRemoteFileUrl).toHaveBeenCalledWith('/repo', 'a.ts', 3)
    expect(result).toBe('https://github.com/o/r/blob/main/a.ts#L3')
  })
})
