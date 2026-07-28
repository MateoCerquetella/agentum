import { api } from '@/tauri'
import { callRuntimeRpc, getActiveRuntimeTarget } from '@/runtime/runtime-rpc-client'
import { useAppStore } from '@/store'

type GitHubProjectOrigin = {
  owner: string
  repo: string
}

function mutationError(error: unknown): string {
  if (typeof error === 'string') return error
  if (error && typeof error === 'object' && 'message' in error) {
    return String((error as { message: unknown }).message)
  }
  return 'GitHub rejected the update.'
}

export async function runPullRequestStateUpdate(args: {
  repoPath: string | null
  repoId?: string | null
  projectOrigin: GitHubProjectOrigin | undefined
  number: number
  updates: { state: 'open' | 'closed' }
}): Promise<void> {
  if (args.projectOrigin) {
    const target = getActiveRuntimeTarget(useAppStore.getState().settings)
    const updateArgs = {
      owner: args.projectOrigin.owner,
      repo: args.projectOrigin.repo,
      number: args.number,
      updates: args.updates
    }
    const result =
      target.kind === 'environment'
        ? await callRuntimeRpc<any>(
            target,
            'github.project.updatePullRequestBySlug',
            updateArgs,
            { timeoutMs: 30_000 }
          )
        : await api.gh.updatePullRequestBySlug(updateArgs)
    if (!result.ok) throw new Error(mutationError(result.error))
    return
  }
  if (!args.repoPath) throw new Error('No repo context available for this pull request.')
  const result = await api.gh.updatePRState({
    repoPath: args.repoPath,
    repoId: args.repoId ?? undefined,
    prNumber: args.number,
    updates: args.updates
  })
  if (!result.ok) throw new Error(mutationError(result.error))
}

export async function runIssueUpdate(args: {
  repoPath: string | null
  repoId?: string | null
  projectOrigin: GitHubProjectOrigin | undefined
  number: number
  updates: Record<string, unknown>
}): Promise<void> {
  if (args.projectOrigin) {
    const target = getActiveRuntimeTarget(useAppStore.getState().settings)
    const updateArgs = {
      owner: args.projectOrigin.owner,
      repo: args.projectOrigin.repo,
      number: args.number,
      updates: args.updates
    }
    const result =
      target.kind === 'environment'
        ? await callRuntimeRpc<any>(target, 'github.project.updateIssueBySlug', updateArgs, {
            timeoutMs: 30_000
          })
        : await api.gh.updateIssueBySlug(updateArgs)
    if (!result.ok) throw new Error(mutationError(result.error))
    return
  }
  if (!args.repoPath) throw new Error('No repo context available for this edit.')
  const result = await api.gh.updateIssue({
    repoPath: args.repoPath,
    repoId: args.repoId ?? undefined,
    number: args.number,
    updates: args.updates
  })
  if (!result.ok) throw new Error(mutationError(result.error))
}
