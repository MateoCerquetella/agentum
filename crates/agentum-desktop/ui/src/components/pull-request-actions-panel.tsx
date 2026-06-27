import { api } from '@/tauri'
import type { PullRequestPageProjectOrigin } from './pull-request-types'
import { WorkItemStateBadge } from './github-item-display'
import React, { useCallback, useState } from 'react'
import { ChevronDown, CircleDot, ExternalLink, GitMerge, GitPullRequest, GitPullRequestClosed, LoaderCircle } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { useConfirmationDialog } from '@/components/confirmation-dialog'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/store'
import { presentGitHubPRMergeState } from '@/components/github-pr-merge-state'
import { GITHUB_PR_MERGE_METHOD_LABELS, resolveGitHubPRMergeMethods } from '../../../shared/github-pr-merge-methods'
import type { GitHubWorkItem, GitHubPRMergeMethod } from '../../../shared/types'

export function PRActionsPanel({
  item,
  repoPath,
  repoId,
  projectOrigin,
  localState,
  onStateChange,
  onMutated
}: {
  item: GitHubWorkItem
  repoPath: string | null
  repoId: string | null
  projectOrigin: PullRequestPageProjectOrigin | undefined
  localState: GitHubWorkItem['state']
  onStateChange: (state: GitHubWorkItem['state']) => void
  onMutated: () => void
}): React.JSX.Element {
  const [statePending, setStatePending] = useState(false)
  const [mergePending, setMergePending] = useState(false)
  const patchWorkItem = useAppStore((s) => s.patchWorkItem)
  const patchProjectRowContent = useAppStore((s) => s.patchProjectRowContent)
  const confirm = useConfirmationDialog()
  const actionItem = { ...item, state: localState }
  const mergePresentation = presentGitHubPRMergeState(actionItem)
  const mergeMethods = resolveGitHubPRMergeMethods(actionItem.mergeMethodSettings)
  const canMutateState = localState !== 'merged' && (!!repoPath || !!projectOrigin)
  const nextState: 'open' | 'closed' = localState === 'closed' ? 'open' : 'closed'
  const mergeDisabled = !repoPath || mergePending || !mergePresentation.directMergeAvailable

  const patchProjectRowIfNeeded = useCallback(
    (state: GitHubWorkItem['state']) => {
      if (!projectOrigin) {
        return
      }
      patchProjectRowContent(projectOrigin.cacheKey, projectOrigin.projectItemId, { state })
    },
    [patchProjectRowContent, projectOrigin]
  )

  const applyStatePatch = useCallback(
    (state: GitHubWorkItem['state']) => {
      onStateChange(state)
      patchWorkItem(item.id, { state }, item.repoId)
      patchProjectRowIfNeeded(state)
    },
    [item.id, item.repoId, onStateChange, patchProjectRowIfNeeded, patchWorkItem]
  )

  const handleStateChange = async (): Promise<void> => {
    if (!canMutateState || statePending) {
      return
    }
    const label = nextState === 'closed' ? 'Close' : 'Reopen'
    const confirmed = await confirm({
      title: `${label} PR #${item.number}?`,
      description:
        nextState === 'closed'
          ? 'This will close the pull request on GitHub.'
          : 'This will reopen the pull request on GitHub.',
      confirmLabel: label,
      confirmVariant: nextState === 'closed' ? 'destructive' : 'default'
    })
    if (!confirmed) {
      return
    }
    const previousState = localState
    setStatePending(true)
    applyStatePatch(nextState)
    try {
      await runPullRequestStateUpdate({
        repoPath,
        repoId,
        projectOrigin,
        number: item.number,
        updates: { state: nextState }
      })
      toast.success(nextState === 'closed' ? 'Pull request closed' : 'Pull request reopened')
      onMutated()
    } catch (err) {
      applyStatePatch(previousState)
      toast.error(err instanceof Error ? err.message : `Failed to ${label.toLowerCase()} PR`)
    } finally {
      setStatePending(false)
    }
  }

  const handleMerge = async (method: GitHubPRMergeMethod): Promise<void> => {
    if (!repoPath || mergeDisabled) {
      return
    }
    const label = GITHUB_PR_MERGE_METHOD_LABELS[method]
    const confirmed = await confirm({
      title: `${label} PR #${item.number}?`,
      description: 'This will update the pull request on GitHub.',
      confirmLabel: label
    })
    if (!confirmed) {
      return
    }
    setMergePending(true)
    try {
      const result = await api.gh.mergePR({
        repoPath,
        repoId: repoId ?? undefined,
        prNumber: item.number,
        method,
        prRepo: item.prRepo ?? null
      })
      if (!result.ok) {
        toast.error(result.error)
        return
      }
      applyStatePatch('merged')
      toast.success('Pull request merged')
      onMutated()
    } catch {
      toast.error('Failed to merge pull request')
    } finally {
      setMergePending(false)
    }
  }

  const handleAutoMerge = async (): Promise<void> => {
    if (!repoPath || !mergePresentation.autoMergeAction) {
      return
    }
    const enabled = mergePresentation.autoMergeAction.kind === 'enable'
    setMergePending(true)
    try {
      const result = await api.gh.setPRAutoMerge({
        repoPath,
        repoId: repoId ?? undefined,
        prNumber: item.number,
        enabled,
        prRepo: item.prRepo ?? null
      })
      if (!result.ok) {
        toast.error(result.error)
        return
      }
      toast.success(enabled ? 'Auto-merge enabled' : 'Auto-merge disabled')
      onMutated()
    } catch {
      toast.error(enabled ? 'Failed to enable auto-merge' : 'Failed to disable auto-merge')
    } finally {
      setMergePending(false)
    }
  }

  return (
    <aside className="rounded-lg border border-border/50 bg-card p-3 shadow-xs">
      <div className="mb-3 flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <GitPullRequest className="size-3.5 text-muted-foreground" />
          <span className="text-[13px] font-medium text-foreground">Pull request</span>
        </div>
        <WorkItemStateBadge item={actionItem} />
      </div>

      <div className="grid gap-2">
        <DropdownMenu modal={false}>
          <Tooltip>
            <TooltipTrigger asChild>
              <DropdownMenuTrigger asChild>
                <Button
                  type="button"
                  size="sm"
                  className={cn(
                    'w-full justify-center gap-2 bg-green-600 text-white hover:bg-green-700',
                    'disabled:cursor-not-allowed disabled:opacity-50'
                  )}
                >
                  {mergePending ? (
                    <LoaderCircle className="size-3.5 animate-spin" />
                  ) : (
                    <GitMerge className="size-3.5" />
                  )}
                  {mergePresentation.autoMergeAction?.label ??
                    (mergePresentation.directMergeAvailable
                      ? mergeMethods.defaultLabel
                      : mergePresentation.label)}
                  <ChevronDown className="size-3 opacity-60" />
                </Button>
              </DropdownMenuTrigger>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={6}>
              {!repoPath ? 'Merge requires a registered local repo' : mergePresentation.tooltip}
            </TooltipContent>
          </Tooltip>
          <DropdownMenuContent align="start" className="w-52">
            {mergePresentation.autoMergeAction && (
              <DropdownMenuItem
                disabled={!repoPath || mergePending}
                onSelect={() => void handleAutoMerge()}
              >
                <GitMerge className="size-4" />
                {mergePresentation.autoMergeAction.label}
              </DropdownMenuItem>
            )}
            {mergePresentation.autoMergeAction && <DropdownMenuSeparator />}
            {mergeMethods.methods.map(({ method, label }) => (
              <DropdownMenuItem
                key={method}
                disabled={mergeDisabled}
                onSelect={() => void handleMerge(method)}
              >
                <GitMerge className="size-4" />
                {label}
              </DropdownMenuItem>
            ))}
            <DropdownMenuItem onSelect={() => api.shell.openUrl(item.url)}>
              <ExternalLink className="size-4" />
              Open GitHub merge box
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>

        <Button
          type="button"
          variant={nextState === 'closed' ? 'outline' : 'secondary'}
          size="sm"
          className={cn(
            'w-full justify-center gap-2',
            nextState === 'closed' &&
              'border-border bg-background text-foreground hover:bg-accent hover:text-accent-foreground dark:border-input dark:bg-input/30 dark:hover:bg-input/50'
          )}
          disabled={!canMutateState || statePending}
          onClick={() => void handleStateChange()}
        >
          {statePending ? (
            <LoaderCircle className="size-3.5 animate-spin" />
          ) : nextState === 'closed' ? (
            <GitPullRequestClosed className="size-3.5 text-destructive" />
          ) : (
            <CircleDot className="size-3.5" />
          )}
          {nextState === 'closed' ? 'Close pull request' : 'Reopen PR'}
        </Button>
      </div>
    </aside>
  )
}
