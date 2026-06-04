/* eslint-disable max-lines -- Why: runtime git routing tests share compatibility-cache and IPC stubs; splitting would hide cross-environment contract drift. */
import { beforeEach, describe, expect, it, vi } from 'vitest'

// Native git was removed: a LOCAL workspace routes through the embedded-server
// adapter, so mock it and assert the local path dispatches there. Remote runtime
// environments still route over RPC (asserted via runtimeEnvironmentCall).
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
  bulkDiscardRuntimeGitPaths,
  bulkStageRuntimeGitPaths,
  cancelRuntimeGenerateCommitMessage,
  commitRuntimeGit,
  discoverRuntimeCommitMessageModels,
  fastForwardRuntimeGit,
  fetchRuntimeGit,
  generateRuntimeCommitMessage,
  getRuntimeGitDiff,
  getRuntimeGitHistory,
  getRuntimeGitIgnoredPaths,
  getRuntimeGitStatus,
  pushRuntimeGit,
  rebaseRuntimeGitFromBase
} from './runtime-git-client'
import { getServerGitStatus } from './server-git-adapter'
import {
  createCompatibleRuntimeStatusResponseIfNeeded,
  type RuntimeEnvironmentCallRequest
} from './runtime-compatibility-test-fixture'
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
  // Native git preload is gone; the RPC path still needs the runtime transports.
  vi.stubGlobal('window', {
    api: {
      runtime: { call: runtimeCall },
      runtimeEnvironments: { call: runtimeEnvironmentTransportCall }
    }
  })
})

describe('runtime git client', () => {
  it('routes a local workspace status through the embedded-server adapter', async () => {
    vi.mocked(getServerGitStatus).mockResolvedValue({ entries: [], conflictOperation: 'unknown' })

    await getRuntimeGitStatus({
      settings: { activeRuntimeEnvironmentId: null },
      worktreeId: 'wt-1',
      worktreePath: '/repo',
      connectionId: 'ssh-1'
    })

    expect(getServerGitStatus).toHaveBeenCalledWith('/repo')
    expect(runtimeEnvironmentCall).not.toHaveBeenCalled()
  })

  it('routes status and diffs through the active runtime environment', async () => {
    runtimeEnvironmentCall.mockResolvedValue({
      id: 'rpc-1',
      ok: true,
      result: { entries: [], conflictOperation: 'unknown' },
      _meta: { runtimeId: 'remote-runtime' }
    })

    await getRuntimeGitStatus({
      settings: { activeRuntimeEnvironmentId: 'env-1' },
      worktreeId: 'wt-1',
      worktreePath: '/repo'
    })
    await getRuntimeGitDiff(
      {
        settings: { activeRuntimeEnvironmentId: 'env-1' },
        worktreeId: 'wt-1',
        worktreePath: '/repo'
      },
      { filePath: 'src/a.ts', staged: false, compareAgainstHead: true }
    )
    await getRuntimeGitHistory(
      {
        settings: { activeRuntimeEnvironmentId: 'env-1' },
        worktreeId: 'wt-1',
        worktreePath: '/repo'
      },
      { limit: 50, baseRef: 'origin/main' }
    )

    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(1, {
      selector: 'env-1',
      method: 'git.status',
      params: { worktree: 'wt-1' },
      timeoutMs: 15_000
    })
    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(2, {
      selector: 'env-1',
      method: 'git.diff',
      params: {
        worktree: 'wt-1',
        filePath: 'src/a.ts',
        staged: false,
        compareAgainstHead: true
      },
      timeoutMs: 15_000
    })
    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(3, {
      selector: 'env-1',
      method: 'git.history',
      params: { worktree: 'wt-1', limit: 50, baseRef: 'origin/main' },
      timeoutMs: 15_000
    })
  })

  it('forwards includeIgnored through the active runtime environment', async () => {
    runtimeEnvironmentCall.mockResolvedValue({
      id: 'rpc-1',
      ok: true,
      result: { entries: [], conflictOperation: 'unknown', ignoredPaths: ['dist/'] },
      _meta: { runtimeId: 'remote-runtime' }
    })

    await getRuntimeGitStatus(
      {
        settings: { activeRuntimeEnvironmentId: 'env-1' },
        worktreeId: 'wt-1',
        worktreePath: '/repo'
      },
      { includeIgnored: true }
    )

    expect(runtimeEnvironmentCall).toHaveBeenCalledWith({
      selector: 'env-1',
      method: 'git.status',
      params: { worktree: 'wt-1', includeIgnored: true },
      timeoutMs: 15_000
    })
  })

  it('checks ignored paths through the active runtime environment', async () => {
    runtimeEnvironmentCall.mockResolvedValue({
      id: 'rpc-1',
      ok: true,
      result: ['dist/bundle.js'],
      _meta: { runtimeId: 'remote-runtime' }
    })

    const result = await getRuntimeGitIgnoredPaths(
      {
        settings: { activeRuntimeEnvironmentId: 'env-1' },
        worktreeId: 'wt-1',
        worktreePath: '/repo'
      },
      ['dist/bundle.js']
    )

    expect(runtimeEnvironmentCall).toHaveBeenCalledWith({
      selector: 'env-1',
      method: 'git.checkIgnored',
      params: { worktree: 'wt-1', paths: ['dist/bundle.js'] },
      timeoutMs: 15_000
    })
    expect(result).toEqual(['dist/bundle.js'])
  })

  it('routes bulk mutations and remote operations through the active runtime', async () => {
    runtimeEnvironmentCall.mockResolvedValue({
      id: 'rpc-1',
      ok: true,
      result: { success: true },
      _meta: { runtimeId: 'remote-runtime' }
    })
    const context = {
      settings: { activeRuntimeEnvironmentId: 'env-1' },
      worktreeId: 'wt-1',
      worktreePath: '/repo'
    }

    await bulkStageRuntimeGitPaths(context, ['a.ts', 'b.ts'])
    await bulkDiscardRuntimeGitPaths(context, ['c.ts', 'd.ts'])
    await commitRuntimeGit(context, 'feat: test')
    await generateRuntimeCommitMessage(context)
    await cancelRuntimeGenerateCommitMessage(context)
    await pushRuntimeGit(context, {
      publish: true,
      pushTarget: { remoteName: 'origin', branchName: 'feature' }
    })
    await fetchRuntimeGit(context, { remoteName: 'fork', branchName: 'feature' })
    await fastForwardRuntimeGit(context, { remoteName: 'fork', branchName: 'feature' })
    await rebaseRuntimeGitFromBase(context, 'origin/main')

    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(1, {
      selector: 'env-1',
      method: 'git.bulkStage',
      params: { worktree: 'wt-1', filePaths: ['a.ts', 'b.ts'] },
      timeoutMs: 15_000
    })
    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(2, {
      selector: 'env-1',
      method: 'git.bulkDiscard',
      params: { worktree: 'wt-1', filePaths: ['c.ts', 'd.ts'] },
      timeoutMs: 15_000
    })
    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(3, {
      selector: 'env-1',
      method: 'git.commit',
      params: { worktree: 'wt-1', message: 'feat: test' },
      timeoutMs: 30_000
    })
    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(4, {
      selector: 'env-1',
      method: 'git.generateCommitMessage',
      params: { worktree: 'wt-1', commitMessageDiscoveryHostKey: 'runtime:env-1' },
      timeoutMs: 75_000
    })
    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(5, {
      selector: 'env-1',
      method: 'git.cancelGenerateCommitMessage',
      params: { worktree: 'wt-1' },
      timeoutMs: 5_000
    })
    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(6, {
      selector: 'env-1',
      method: 'git.push',
      params: {
        worktree: 'wt-1',
        publish: true,
        pushTarget: { remoteName: 'origin', branchName: 'feature' }
      },
      timeoutMs: 30_000
    })
    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(7, {
      selector: 'env-1',
      method: 'git.fetch',
      params: {
        worktree: 'wt-1',
        pushTarget: { remoteName: 'fork', branchName: 'feature' }
      },
      timeoutMs: 30_000
    })
    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(8, {
      selector: 'env-1',
      method: 'git.fastForward',
      params: {
        worktree: 'wt-1',
        pushTarget: { remoteName: 'fork', branchName: 'feature' }
      },
      timeoutMs: 30_000
    })
    expect(runtimeEnvironmentCall).toHaveBeenNthCalledWith(9, {
      selector: 'env-1',
      method: 'git.rebaseFromBase',
      params: { worktree: 'wt-1', baseRef: 'origin/main' },
      timeoutMs: 30_000
    })
  })

  it('passes commit-message settings to the active runtime', async () => {
    const commitMessageAi = {
      enabled: true,
      agentId: 'codex' as const,
      selectedModelByAgent: { codex: 'gpt-5.3-codex-spark' },
      selectedThinkingByModel: { 'gpt-5.3-codex-spark': 'medium' },
      customPrompt: 'Prefer concise subjects.',
      customAgentCommand: ''
    }
    const agentCmdOverrides = { codex: 'codex --profile work' }
    runtimeEnvironmentCall.mockResolvedValue({
      id: 'rpc-1',
      ok: true,
      result: { success: true, message: 'feat: test' },
      _meta: { runtimeId: 'remote-runtime' }
    })

    await generateRuntimeCommitMessage({
      settings: {
        activeRuntimeEnvironmentId: 'env-1',
        commitMessageAi,
        agentCmdOverrides,
        enableGitHubAttribution: true
      },
      worktreeId: 'wt-1',
      worktreePath: '/repo'
    })

    expect(runtimeEnvironmentCall).toHaveBeenCalledWith({
      selector: 'env-1',
      method: 'git.generateCommitMessage',
      params: {
        worktree: 'wt-1',
        commitMessageAi,
        agentCmdOverrides,
        enableGitHubAttribution: true,
        commitMessageDiscoveryHostKey: 'runtime:env-1'
      },
      timeoutMs: 75_000
    })
  })

  it('discovers commit-message models through the active runtime', async () => {
    const agentCmdOverrides = { cursor: 'cursor-agent' }
    runtimeEnvironmentCall.mockResolvedValue({
      id: 'rpc-1',
      ok: true,
      result: { success: true, models: [{ id: 'auto', label: 'Auto' }], defaultModelId: 'auto' },
      _meta: { runtimeId: 'remote-runtime' }
    })

    await discoverRuntimeCommitMessageModels(
      {
        settings: { activeRuntimeEnvironmentId: 'env-1', agentCmdOverrides },
        worktreeId: 'wt-1',
        worktreePath: '/repo'
      },
      'cursor'
    )

    expect(runtimeEnvironmentCall).toHaveBeenCalledWith({
      selector: 'env-1',
      method: 'git.discoverCommitMessageModels',
      params: { worktree: 'wt-1', agentId: 'cursor', agentCmdOverrides },
      timeoutMs: 75_000
    })
  })

  it('returns the no-runtime stub for local commit-message generation', async () => {
    const result = await generateRuntimeCommitMessage({
      settings: { activeRuntimeEnvironmentId: null },
      worktreeId: 'wt-1',
      worktreePath: '/repo'
    })
    expect(result.success).toBe(false)
    expect(runtimeEnvironmentCall).not.toHaveBeenCalled()
  })
})
