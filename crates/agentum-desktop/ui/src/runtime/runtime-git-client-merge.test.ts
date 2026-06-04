import { beforeEach, describe, expect, it, vi } from 'vitest'

// Native git was removed: a local workspace's abort routes through the
// embedded-server adapter; a remote runtime environment routes over RPC.
vi.mock('./server-git-adapter', () => ({
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
  getServerGitDiff: vi.fn(),
  getServerGitCheckIgnored: vi.fn(),
  serverGitFastForward: vi.fn(),
  getServerGitRemoteFileUrl: vi.fn(),
  getServerGitCommitDiff: vi.fn(),
  getServerGitBranchDiff: vi.fn(),
  getServerGitHistory: vi.fn()
}))

import {
  createCompatibleRuntimeStatusResponseIfNeeded,
  type RuntimeEnvironmentCallRequest
} from './runtime-compatibility-test-fixture'
import { abortRuntimeGitMerge, abortRuntimeGitRebase } from './runtime-git-client'
import { serverGitAbortMerge, serverGitAbortRebase } from './server-git-adapter'
import { clearRuntimeCompatibilityCacheForTests } from './runtime-rpc-client'

const runtimeEnvironmentCall = vi.fn()
const runtimeEnvironmentTransportCall = vi.fn()
const runtimeCall = vi.fn()

beforeEach(() => {
  clearRuntimeCompatibilityCacheForTests()
  vi.clearAllMocks()
  runtimeEnvironmentTransportCall.mockImplementation((args: RuntimeEnvironmentCallRequest) => {
    return createCompatibleRuntimeStatusResponseIfNeeded(args) ?? runtimeEnvironmentCall(args)
  })
  vi.stubGlobal('window', {
    api: {
      runtime: { call: runtimeCall },
      runtimeEnvironments: { call: runtimeEnvironmentTransportCall }
    }
  })
})

describe('runtime git client merge operations', () => {
  it('aborts a merge through the embedded-server adapter for a local workspace', async () => {
    vi.mocked(serverGitAbortMerge).mockResolvedValue()

    await abortRuntimeGitMerge({
      settings: { activeRuntimeEnvironmentId: null },
      worktreeId: 'wt-1',
      worktreePath: '/repo'
    })

    expect(serverGitAbortMerge).toHaveBeenCalledWith('/repo')
    expect(runtimeEnvironmentCall).not.toHaveBeenCalled()
  })

  it('routes abort merge through the active runtime', async () => {
    runtimeEnvironmentCall.mockResolvedValue({
      id: 'rpc-1',
      ok: true,
      result: { success: true },
      _meta: { runtimeId: 'remote-runtime' }
    })

    await abortRuntimeGitMerge({
      settings: { activeRuntimeEnvironmentId: 'env-1' },
      worktreeId: 'wt-1',
      worktreePath: '/repo'
    })

    expect(runtimeEnvironmentCall).toHaveBeenCalledWith({
      selector: 'env-1',
      method: 'git.abortMerge',
      params: { worktree: 'wt-1' },
      timeoutMs: 30_000
    })
    expect(serverGitAbortMerge).not.toHaveBeenCalled()
  })

  it('aborts a rebase through the embedded-server adapter for a local workspace', async () => {
    vi.mocked(serverGitAbortRebase).mockResolvedValue()

    await abortRuntimeGitRebase({
      settings: { activeRuntimeEnvironmentId: null },
      worktreeId: 'wt-1',
      worktreePath: '/repo',
      connectionId: 'ssh-1'
    })

    expect(serverGitAbortRebase).toHaveBeenCalledWith('/repo')
    expect(runtimeEnvironmentCall).not.toHaveBeenCalled()
  })

  it('routes abort rebase through the active runtime', async () => {
    runtimeEnvironmentCall.mockResolvedValue({
      id: 'rpc-1',
      ok: true,
      result: { success: true },
      _meta: { runtimeId: 'remote-runtime' }
    })

    await abortRuntimeGitRebase({
      settings: { activeRuntimeEnvironmentId: 'env-1' },
      worktreeId: 'wt-1',
      worktreePath: '/repo'
    })

    expect(runtimeEnvironmentCall).toHaveBeenCalledWith({
      selector: 'env-1',
      method: 'git.abortRebase',
      params: { worktree: 'wt-1' },
      timeoutMs: 30_000
    })
    expect(serverGitAbortRebase).not.toHaveBeenCalled()
  })
})
