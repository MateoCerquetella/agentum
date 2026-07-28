/* eslint-disable max-lines -- Why: this module mirrors the git API with
runtime-aware routing so source-control callers have one typed boundary instead
of reimplementing local-vs-environment branching per operation. */
import type {
  GitBranchCompareResult,
  GitCommitCompareResult,
  GitConflictOperation,
  GitDiffResult,
  GitPushTarget,
  GitStatusResult,
  GitUpstreamStatus,
  GlobalSettings
} from '@/shared/types'
import type {
  CommitMessageAgentCapability,
  CommitMessageModelCapability
} from '@/shared/commit-message-agent-spec'
import { getCommitMessageModelDiscoveryHostKeyForScope } from '@/shared/commit-message-host-key'
import type { GitHistoryOptions, GitHistoryResult } from '@/shared/git-history'
import { callRuntimeRpc, getActiveRuntimeTarget } from './runtime-rpc-client'
import {
  getServerGitStatus,
  getServerGitConflictOperation,
  getServerGitUpstreamStatus,
  serverGitStage,
  getServerGitBranchCompare,
  getServerGitCommitCompare,
  serverGitFetch,
  serverGitPull,
  serverGitCommit,
  serverGitDiscard,
  serverGitPush,
  serverGitRebase,
  serverGitAbortMerge,
  serverGitAbortRebase,
  getServerGitDiff,
  getServerGitCheckIgnored,
  serverGitFastForward,
  getServerGitRemoteFileUrl,
  getServerGitCommitDiff,
  getServerGitBranchDiff,
  getServerGitHistory
} from './server-git-adapter'

/**
 * Source-control routing for the desktop. A LOCAL workspace's git runs against
 * its embedded-server session (`server-git-adapter`); a remote runtime
 * environment routes over RPC. There is no longer a native (Tauri) git preload:
 * the embedded server is the single git surface (the duplicate `commands/git.rs`
 * was removed). `target.kind === 'local' || !worktreeId` selects the local
 * server path; everything else is an active runtime environment (RPC).
 */

export type RuntimeGenerateCommitMessageResult =
  | { success: true; message: string; agentLabel?: string }
  | { success: false; error: string; canceled?: boolean }

export type RuntimeGeneratePullRequestFieldsResult =
  | {
      success: true
      fields: { base: string; title: string; body: string; draft: boolean }
      agentLabel?: string
      branchChangedByPreparation?: boolean
    }
  | { success: false; error: string; canceled?: boolean; branchChangedByPreparation?: boolean }

type RuntimeGitSettings = Pick<GlobalSettings, 'activeRuntimeEnvironmentId'> &
  Partial<
    Pick<
      GlobalSettings,
      'commitMessageAi' | 'sourceControlAi' | 'agentCmdOverrides' | 'enableGitHubAttribution'
    >
  >

type RuntimeDiscoverCommitMessageModelsResult =
  | {
      success: true
      capability: CommitMessageAgentCapability
      models: CommitMessageModelCapability[]
      defaultModelId: string
    }
  | { success: false; error: string }

export type RuntimeGitContext = {
  settings: RuntimeGitSettings | null | undefined
  worktreeId: string | null | undefined
  worktreePath: string
  connectionId?: string
}

function getRuntimeCommitMessageSettings(
  settings: RuntimeGitSettings | null | undefined,
  connectionId?: string
): Partial<
  Pick<
    GlobalSettings,
    'commitMessageAi' | 'sourceControlAi' | 'agentCmdOverrides' | 'enableGitHubAttribution'
  >
> & {
  commitMessageDiscoveryHostKey?: string
} {
  if (!settings) {
    return {}
  }
  const scope = getRuntimeGitScope(settings, connectionId)
  return {
    ...(settings.commitMessageAi !== undefined
      ? { commitMessageAi: settings.commitMessageAi }
      : {}),
    ...(settings.sourceControlAi !== undefined
      ? { sourceControlAi: settings.sourceControlAi }
      : {}),
    ...(settings.agentCmdOverrides !== undefined
      ? { agentCmdOverrides: settings.agentCmdOverrides }
      : {}),
    ...(settings.enableGitHubAttribution !== undefined
      ? { enableGitHubAttribution: settings.enableGitHubAttribution }
      : {}),
    commitMessageDiscoveryHostKey: getCommitMessageModelDiscoveryHostKeyForScope(scope)
  }
}

export function getRuntimeGitScope(
  settings: Pick<GlobalSettings, 'activeRuntimeEnvironmentId'> | null | undefined,
  connectionId: string | null | undefined
): string | null | undefined {
  const target = getActiveRuntimeTarget(settings)
  return target.kind === 'environment' ? `runtime:${target.environmentId}` : connectionId
}

/** True for a local workspace (or a workspace with no runtime worktree id): its
 *  git runs against the embedded server, not a remote runtime environment. */
function isLocalGit(
  target: ReturnType<typeof getActiveRuntimeTarget>,
  context: RuntimeGitContext
): boolean {
  return target.kind === 'local' || !context.worktreeId
}

export async function getRuntimeGitStatus(
  context: RuntimeGitContext,
  options?: { includeIgnored?: boolean }
): Promise<GitStatusResult> {
  const target = getActiveRuntimeTarget(context.settings)
  const includeIgnoredArgs = options?.includeIgnored ? { includeIgnored: true } : {}
  if (isLocalGit(target, context)) {
    // The server status doesn't fold in ignoredPaths; callers that need them use
    // getRuntimeGitIgnoredPaths (a separate check-ignore call).
    return getServerGitStatus(context.worktreePath)
  }
  return callRuntimeRpc<GitStatusResult>(
    target,
    'git.status',
    { worktree: context.worktreeId, ...includeIgnoredArgs },
    { timeoutMs: 15_000 }
  )
}

export async function getRuntimeGitIgnoredPaths(
  context: RuntimeGitContext,
  paths: string[]
): Promise<string[]> {
  const target = getActiveRuntimeTarget(context.settings)
  if (paths.length === 0) {
    return []
  }
  if (isLocalGit(target, context)) {
    return getServerGitCheckIgnored(context.worktreePath, paths)
  }
  return callRuntimeRpc<string[]>(
    target,
    'git.checkIgnored',
    { worktree: context.worktreeId, paths },
    { timeoutMs: 15_000 }
  )
}

export async function getRuntimeGitHistory(
  context: RuntimeGitContext,
  options: GitHistoryOptions = {}
): Promise<GitHistoryResult> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return getServerGitHistory(context.worktreePath, options)
  }
  return callRuntimeRpc<GitHistoryResult>(
    target,
    'git.history',
    { worktree: context.worktreeId, ...options },
    { timeoutMs: 15_000 }
  )
}

export async function getRuntimeGitConflictOperation(
  context: RuntimeGitContext
): Promise<GitConflictOperation> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return getServerGitConflictOperation(context.worktreePath)
  }
  return callRuntimeRpc<GitConflictOperation>(
    target,
    'git.conflictOperation',
    { worktree: context.worktreeId },
    { timeoutMs: 15_000 }
  )
}

export async function abortRuntimeGitMerge(context: RuntimeGitContext): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    await serverGitAbortMerge(context.worktreePath)
    return
  }
  await callRuntimeRpc(
    target,
    'git.abortMerge',
    { worktree: context.worktreeId },
    { timeoutMs: 30_000 }
  )
}

export async function abortRuntimeGitRebase(context: RuntimeGitContext): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    await serverGitAbortRebase(context.worktreePath)
    return
  }
  await callRuntimeRpc(
    target,
    'git.abortRebase',
    { worktree: context.worktreeId },
    { timeoutMs: 30_000 }
  )
}

export async function getRuntimeGitDiff(
  context: RuntimeGitContext,
  args: { filePath: string; staged: boolean; compareAgainstHead?: boolean }
): Promise<GitDiffResult> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return getServerGitDiff(context.worktreePath, args)
  }
  return callRuntimeRpc<GitDiffResult>(
    target,
    'git.diff',
    { worktree: context.worktreeId, ...args },
    { timeoutMs: 15_000 }
  )
}

export async function getRuntimeGitBranchCompare(
  context: RuntimeGitContext,
  baseRef: string
): Promise<GitBranchCompareResult> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return getServerGitBranchCompare(context.worktreePath, baseRef)
  }
  return callRuntimeRpc<GitBranchCompareResult>(
    target,
    'git.branchCompare',
    { worktree: context.worktreeId, baseRef },
    { timeoutMs: 15_000 }
  )
}

export async function getRuntimeGitCommitCompare(
  context: RuntimeGitContext,
  commitId: string
): Promise<GitCommitCompareResult> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return getServerGitCommitCompare(context.worktreePath, commitId)
  }
  return callRuntimeRpc<GitCommitCompareResult>(
    target,
    'git.commitCompare',
    { worktree: context.worktreeId, commitId },
    { timeoutMs: 15_000 }
  )
}

export async function getRuntimeGitUpstreamStatus(
  context: RuntimeGitContext,
  pushTarget?: GitPushTarget
): Promise<GitUpstreamStatus> {
  const target = getActiveRuntimeTarget(context.settings)
  // The embedded server tracks @{u} only — it ignores an explicit pushTarget,
  // matching the prior native command (whose signature had no pushTarget either).
  if (isLocalGit(target, context)) {
    return getServerGitUpstreamStatus(context.worktreePath)
  }
  return callRuntimeRpc<GitUpstreamStatus>(
    target,
    'git.upstreamStatus',
    { worktree: context.worktreeId, ...(pushTarget ? { pushTarget } : {}) },
    { timeoutMs: 15_000 }
  )
}

export async function fetchRuntimeGit(
  context: RuntimeGitContext,
  pushTarget?: GitPushTarget
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  // Embedded server fetch is `fetch --all --prune` (target-agnostic).
  if (isLocalGit(target, context)) {
    await serverGitFetch(context.worktreePath)
    return
  }
  await callRuntimeRpc(
    target,
    'git.fetch',
    { worktree: context.worktreeId, ...(pushTarget ? { pushTarget } : {}) },
    { timeoutMs: 30_000 }
  )
}

export async function pullRuntimeGit(
  context: RuntimeGitContext,
  pushTarget?: GitPushTarget
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  // Embedded server pull is fast-forward-only (target-agnostic).
  if (isLocalGit(target, context)) {
    await serverGitPull(context.worktreePath)
    return
  }
  await callRuntimeRpc(
    target,
    'git.pull',
    { worktree: context.worktreeId, ...(pushTarget ? { pushTarget } : {}) },
    { timeoutMs: 30_000 }
  )
}

export async function fastForwardRuntimeGit(
  context: RuntimeGitContext,
  pushTarget?: GitPushTarget
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  // Embedded server fast-forwards to @{upstream} (target-agnostic).
  if (isLocalGit(target, context)) {
    await serverGitFastForward(context.worktreePath)
    return
  }
  await callRuntimeRpc(
    target,
    'git.fastForward',
    { worktree: context.worktreeId, ...(pushTarget ? { pushTarget } : {}) },
    { timeoutMs: 30_000 }
  )
}

export async function rebaseRuntimeGitFromBase(
  context: RuntimeGitContext,
  baseRef: string
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    await serverGitRebase(context.worktreePath, baseRef)
    return
  }
  await callRuntimeRpc(
    target,
    'git.rebaseFromBase',
    { worktree: context.worktreeId, baseRef },
    { timeoutMs: 30_000 }
  )
}

export async function pushRuntimeGit(
  context: RuntimeGitContext,
  args: { publish?: boolean; pushTarget?: GitPushTarget; forceWithLease?: boolean } = {}
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  // Embedded server push is `push --set-upstream origin HEAD` — it does not model
  // publish / a specific pushTarget / force-with-lease (neither did the prior
  // native command, whose signature had none of them). Remote runtime
  // environments still honour them over RPC.
  if (isLocalGit(target, context)) {
    await serverGitPush(context.worktreePath)
    return
  }
  await callRuntimeRpc(
    target,
    'git.push',
    {
      worktree: context.worktreeId,
      ...(args.publish !== undefined ? { publish: args.publish } : {}),
      ...(args.pushTarget !== undefined ? { pushTarget: args.pushTarget } : {}),
      ...(args.forceWithLease !== undefined ? { forceWithLease: args.forceWithLease } : {})
    },
    { timeoutMs: 30_000 }
  )
}

export async function getRuntimeGitBranchDiff(
  context: RuntimeGitContext,
  args: {
    compare: { baseRef: string; baseOid: string; headOid: string; mergeBase: string }
    filePath: string
    oldPath?: string
  }
): Promise<GitDiffResult> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return getServerGitBranchDiff(context.worktreePath, args)
  }
  return callRuntimeRpc<GitDiffResult>(
    target,
    'git.branchDiff',
    { worktree: context.worktreeId, ...args },
    { timeoutMs: 15_000 }
  )
}

export async function getRuntimeGitCommitDiff(
  context: RuntimeGitContext,
  args: {
    commitOid: string
    parentOid?: string | null
    filePath: string
    oldPath?: string
  }
): Promise<GitDiffResult> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return getServerGitCommitDiff(context.worktreePath, args)
  }
  return callRuntimeRpc<GitDiffResult>(
    target,
    'git.commitDiff',
    { worktree: context.worktreeId, ...args },
    { timeoutMs: 15_000 }
  )
}

export async function commitRuntimeGit(
  context: RuntimeGitContext,
  message: string
): Promise<{ success: boolean; error?: string }> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return serverGitCommit(context.worktreePath, message)
  }
  return callRuntimeRpc<{ success: boolean; error?: string }>(
    target,
    'git.commit',
    { worktree: context.worktreeId, message },
    { timeoutMs: 30_000 }
  )
}

export async function generateRuntimeCommitMessage(
  context: RuntimeGitContext
): Promise<RuntimeGenerateCommitMessageResult> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    // The desktop doesn't host the agent runtime locally (the prior native
    // command was a fixed failure stub); return that contract variant.
    return {
      success: false,
      error: "Commit-message generation requires the agent runtime, which isn't available yet."
    }
  }
  return callRuntimeRpc<RuntimeGenerateCommitMessageResult>(
    target,
    'git.generateCommitMessage',
    {
      worktree: context.worktreeId,
      ...getRuntimeCommitMessageSettings(context.settings, context.connectionId)
    },
    { timeoutMs: 75_000 }
  )
}

export async function discoverRuntimeCommitMessageModels(
  context: RuntimeGitContext,
  agentId: string
): Promise<RuntimeDiscoverCommitMessageModelsResult> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return { success: false, error: 'No commit-message models available.' }
  }
  return callRuntimeRpc<RuntimeDiscoverCommitMessageModelsResult>(
    target,
    'git.discoverCommitMessageModels',
    {
      worktree: context.worktreeId,
      agentId,
      ...(context.settings?.agentCmdOverrides
        ? { agentCmdOverrides: context.settings.agentCmdOverrides }
        : {})
    },
    { timeoutMs: 75_000 }
  )
}

export async function cancelRuntimeGenerateCommitMessage(
  context: RuntimeGitContext
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    // No local agent runtime to cancel (the prior native command was a no-op).
    return
  }
  await callRuntimeRpc(
    target,
    'git.cancelGenerateCommitMessage',
    { worktree: context.worktreeId },
    { timeoutMs: 5_000 }
  )
}

export async function generateRuntimePullRequestFields(
  context: RuntimeGitContext,
  input: { base: string; title: string; body: string; draft: boolean }
): Promise<RuntimeGeneratePullRequestFieldsResult> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return {
      success: false,
      error: "PR-field generation requires the agent runtime, which isn't available yet."
    }
  }
  return callRuntimeRpc<RuntimeGeneratePullRequestFieldsResult>(
    target,
    'git.generatePullRequestFields',
    {
      worktree: context.worktreeId,
      ...input,
      ...getRuntimeCommitMessageSettings(context.settings, context.connectionId)
    },
    { timeoutMs: 75_000 }
  )
}

export async function cancelRuntimeGeneratePullRequestFields(
  context: RuntimeGitContext
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return
  }
  await callRuntimeRpc(
    target,
    'git.cancelGeneratePullRequestFields',
    { worktree: context.worktreeId },
    { timeoutMs: 5_000 }
  )
}

export async function stageRuntimeGitPath(
  context: RuntimeGitContext,
  filePath: string
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    await serverGitStage(context.worktreePath, [filePath], false)
    return
  }
  await callRuntimeRpc(
    target,
    'git.stage',
    { worktree: context.worktreeId, filePath },
    { timeoutMs: 15_000 }
  )
}

export async function bulkStageRuntimeGitPaths(
  context: RuntimeGitContext,
  filePaths: string[]
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    await serverGitStage(context.worktreePath, filePaths, false)
    return
  }
  await callRuntimeRpc(
    target,
    'git.bulkStage',
    { worktree: context.worktreeId, filePaths },
    { timeoutMs: 15_000 }
  )
}

export async function unstageRuntimeGitPath(
  context: RuntimeGitContext,
  filePath: string
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    await serverGitStage(context.worktreePath, [filePath], true)
    return
  }
  await callRuntimeRpc(
    target,
    'git.unstage',
    { worktree: context.worktreeId, filePath },
    { timeoutMs: 15_000 }
  )
}

export async function bulkUnstageRuntimeGitPaths(
  context: RuntimeGitContext,
  filePaths: string[]
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    await serverGitStage(context.worktreePath, filePaths, true)
    return
  }
  await callRuntimeRpc(
    target,
    'git.bulkUnstage',
    { worktree: context.worktreeId, filePaths },
    { timeoutMs: 15_000 }
  )
}

export async function bulkDiscardRuntimeGitPaths(
  context: RuntimeGitContext,
  filePaths: string[]
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    await serverGitDiscard(context.worktreePath, filePaths)
    return
  }
  await callRuntimeRpc(
    target,
    'git.bulkDiscard',
    { worktree: context.worktreeId, filePaths },
    { timeoutMs: 15_000 }
  )
}

export async function discardRuntimeGitPath(
  context: RuntimeGitContext,
  filePath: string
): Promise<void> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    await serverGitDiscard(context.worktreePath, [filePath])
    return
  }
  await callRuntimeRpc(
    target,
    'git.discard',
    { worktree: context.worktreeId, filePath },
    { timeoutMs: 15_000 }
  )
}

export async function getRuntimeGitRemoteFileUrl(
  context: RuntimeGitContext,
  args: { relativePath: string; line: number }
): Promise<string | null> {
  const target = getActiveRuntimeTarget(context.settings)
  if (isLocalGit(target, context)) {
    return getServerGitRemoteFileUrl(context.worktreePath, args.relativePath, args.line)
  }
  return callRuntimeRpc<string | null>(
    target,
    'git.remoteFileUrl',
    { worktree: context.worktreeId, relativePath: args.relativePath, line: args.line },
    { timeoutMs: 15_000 }
  )
}
