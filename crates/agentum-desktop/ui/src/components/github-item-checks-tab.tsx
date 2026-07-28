import { api } from '@/tauri'
import { parseOwnerRepoFromItemUrl } from '@/lib/github-item-url'
import { CHECK_SORT_ORDER, formatCheckTimestamp, getCheckConclusion, getCheckCounts, getCheckStatusLabel, getChecksSummaryLabel } from '@/lib/pr-check-format'
import { getBrokenChecks } from './pr-checks-fix-prompt'
import { getCheckDetailsKey } from '@/lib/github-pr-detail-helpers'
import React, { useCallback, useMemo, useState } from 'react'
import { Check, ChevronDown, CircleDashed, ExternalLink, LoaderCircle, RefreshCw, Settings, Wrench } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { useMountedRef } from '@/hooks/useMountedRef'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import CommentMarkdown from '@/components/sidebar/CommentMarkdown'
import { cn } from '@/lib/utils'
import { CHECK_COLOR, CHECK_ICON } from '@/components/right-sidebar/checks-panel-content'
import { createGitHubChecksTabState, resolveGitHubChecksTabState, toggleGitHubChecksTabExpandedKey, updateGitHubChecksTabDetails, updateGitHubChecksTabLocalChecks, type CheckDetailsLoadState } from '@/components/github-checks-tab-state'
import { useAppStore } from '@/store'
import { pickDefaultAgent } from '@/lib/agent-catalog'
import { getConnectionId } from '@/lib/connection-context'
import { focusTerminalTabSurface } from '@/lib/focus-terminal-tab-surface'
import { findGithubPrWorkspaceAttachment } from '@/lib/github-work-item-workspace-attachment'
import { launchAgentInNewTab } from '@/lib/launch-agent-in-new-tab'
import { requestNewSpecFromWorkItem } from '@/lib/sdd-new-spec-entry'
import { activateAndRevealWorktree } from '@/lib/worktree-activation'
import type { GitHubWorkItem, GitHubWorkItemDetails, PRCheckDetail } from '@/shared/types'

function buildFixBrokenChecksPrompt(item: GitHubWorkItem, checks: PRCheckDetail[]): string {
  const brokenChecks = getBrokenChecks(checks)
  const checkLines =
    brokenChecks.length > 0
      ? brokenChecks.map((check) => {
          const details = [
            getCheckStatusLabel(check),
            check.checkRunId ? `check run ${check.checkRunId}` : null,
            check.workflowRunId ? `workflow run ${check.workflowRunId}` : null,
            check.url ? `details: ${check.url}` : null
          ]
            .filter(Boolean)
            .join(', ')
          return `- ${check.name}${details ? ` (${details})` : ''}`
        })
      : ['- No failing check is currently listed; refresh PR checks first, then inspect CI.']

  return [
    `Fix the broken checks for PR #${item.number}: ${item.title}`,
    `PR: ${item.url}`,
    '',
    'Broken checks:',
    ...checkLines,
    '',
    'Focus only on making the failing checks pass. Inspect the CI output first, make the smallest correct code or test changes, and do not work on unrelated cleanup.'
  ].join('\n')
}

export function ChecksTab({
  item,
  repoPath,
  repoId,
  headSha,
  checks,
  loading,
  variant = 'compact',
  onChecksUpdated
}: {
  item: GitHubWorkItem
  repoPath: string | null
  repoId: string | null
  headSha: string | undefined
  checks: GitHubWorkItemDetails['checks']
  loading: boolean
  variant?: 'compact' | 'page'
  onChecksUpdated: (checks: PRCheckDetail[]) => void
}): React.JSX.Element {
  const [refreshing, setRefreshing] = useState(false)
  const [rerunning, setRerunning] = useState(false)
  const [fixingChecks, setFixingChecks] = useState(false)
  const [checksState, setChecksState] = useState(() => createGitHubChecksTabState(checks))
  const mountedRef = useMountedRef()
  const resolvedChecksState = resolveGitHubChecksTabState(checksState, checks)
  if (resolvedChecksState !== checksState) {
    // Why: parent check refreshes replace the source list; clear local refresh
    // and inline detail state before stale rows/details can paint.
    setChecksState(resolvedChecksState)
  }
  const { localChecks, expandedCheckKey, detailsByCheckKey } = resolvedChecksState
  const list = useMemo(() => localChecks ?? checks ?? [], [checks, localChecks])
  const prRepo = useMemo(() => parseOwnerRepoFromItemUrl(item.url), [item.url])
  const sorted = [...list].sort(
    (a, b) =>
      (CHECK_SORT_ORDER[getCheckConclusion(a)] ?? 3) -
      (CHECK_SORT_ORDER[getCheckConclusion(b)] ?? 3)
  )
  const failedChecks = getBrokenChecks(list)
  const counts = getCheckCounts(list)
  const summaryLabel = getChecksSummaryLabel(list)
  const SummaryIcon =
    counts.failing > 0
      ? CHECK_ICON.failure
      : counts.pending > 0
        ? CHECK_ICON.pending
        : list.length > 0
          ? CHECK_ICON.success
          : CircleDashed
  const summaryColor =
    counts.failing > 0
      ? CHECK_COLOR.failure
      : counts.pending > 0
        ? CHECK_COLOR.pending
        : list.length > 0
          ? CHECK_COLOR.success
          : 'text-muted-foreground'
  const canFixBrokenChecks = Boolean((repoId ?? item.repoId) && failedChecks.length > 0)

  const handleRefresh = useCallback(async (): Promise<PRCheckDetail[] | null> => {
    if (!repoPath) {
      toast.error('Unable to refresh checks without a repository path.')
      return null
    }
    setRefreshing(true)
    try {
      const nextChecks = (await api.gh.prChecks({
        repoPath,
        repoId: repoId ?? undefined,
        prNumber: item.number,
        headSha,
        noCache: true
      })) as PRCheckDetail[]
      setChecksState((current) => updateGitHubChecksTabLocalChecks(current, nextChecks))
      onChecksUpdated(nextChecks)
      return nextChecks
    } catch (err) {
      toast.error(err instanceof Error ? err.message : 'Failed to refresh checks')
      return null
    } finally {
      setRefreshing(false)
    }
  }, [headSha, item.number, onChecksUpdated, repoId, repoPath])

  const handleRerun = useCallback(
    async (failedOnly: boolean): Promise<void> => {
      if (!repoPath || rerunning) {
        return
      }
      setRerunning(true)
      try {
        const result = await api.gh.rerunPRChecks({
          repoPath,
          repoId: repoId ?? undefined,
          prNumber: item.number,
          headSha,
          failedOnly
        })
        if (!result.ok) {
          toast.error(result.error)
          return
        }
        toast.success(result.count === 1 ? 'Check rerun requested' : 'Check reruns requested')
        await handleRefresh()
      } catch (err) {
        toast.error(err instanceof Error ? err.message : 'Failed to rerun checks')
      } finally {
        setRerunning(false)
      }
    },
    [handleRefresh, headSha, item.number, rerunning, repoId, repoPath]
  )

  const handleFixBrokenChecks = useCallback(async (): Promise<void> => {
    const targetRepoId = repoId ?? item.repoId
    if (!targetRepoId || fixingChecks) {
      return
    }
    if (failedChecks.length === 0) {
      toast.message('No broken checks to fix.')
      return
    }

    setFixingChecks(true)
    try {
      const prompt = buildFixBrokenChecksPrompt(item, list)
      const store = useAppStore.getState()
      const attachedWorkspace = findGithubPrWorkspaceAttachment(
        store.allWorktrees(),
        targetRepoId,
        item.number
      )

      if (!attachedWorkspace) {
        requestNewSpecFromWorkItem({
          repoId: targetRepoId,
          title: `Fix broken checks for ${item.title}`,
          provider: 'github',
          reference: item.url,
          goal: prompt
        })
        toast.message('Review the pull request in New Spec before implementation.')
        return
      }

      if (!activateAndRevealWorktree(attachedWorkspace.id)) {
        toast.error('Unable to open the workspace attached to this pull request.')
        return
      }

      const connectionId = getConnectionId(attachedWorkspace.id)
      if (connectionId === undefined) {
        toast.error('Unable to resolve the workspace connection.')
        return
      }

      const activeStore = useAppStore.getState()
      const detectedAgents =
        typeof connectionId === 'string'
          ? await activeStore.ensureRemoteDetectedAgents(connectionId)
          : await activeStore.ensureDetectedAgents()
      const agent = pickDefaultAgent(
        activeStore.settings?.defaultTuiAgent,
        detectedAgents,
        activeStore.settings?.disabledTuiAgents
      )
      if (!agent) {
        toast.error('No enabled AI agents. Configure agents in Settings.')
        return
      }

      const result = launchAgentInNewTab({
        agent,
        worktreeId: attachedWorkspace.id,
        prompt,
        promptDelivery: 'draft',
        launchSource: 'task_page'
      })
      if (!result) {
        toast.error('Could not build the agent launch command.')
        return
      }
      focusTerminalTabSurface(result.tabId)
      toast.success('Started an AI agent for the broken checks.')
    } finally {
      setFixingChecks(false)
    }
  }, [failedChecks.length, fixingChecks, item, list, repoId])

  const handleToggleCheckDetails = useCallback(
    (check: PRCheckDetail): void => {
      const key = getCheckDetailsKey(check)
      setChecksState((current) => toggleGitHubChecksTabExpandedKey(current, key))
      if (
        !repoPath ||
        detailsByCheckKey[key] ||
        (!check.checkRunId && !check.workflowRunId && !check.url)
      ) {
        return
      }
      setChecksState((current) =>
        updateGitHubChecksTabDetails(current, key, { loading: true, details: null, error: null })
      )
      void api.gh
        .prCheckDetails({
          repoPath,
          repoId: repoId ?? undefined,
          checkRunId: check.checkRunId,
          workflowRunId: check.workflowRunId,
          checkName: check.name,
          url: check.url,
          prRepo
        })
        .then((details) => {
          if (!mountedRef.current) {
            return
          }
          setChecksState((current) =>
            updateGitHubChecksTabDetails(current, key, {
              loading: false,
              details,
              error: details ? null : 'No inline details are available for this check.'
            })
          )
        })
        .catch((err) => {
          if (!mountedRef.current) {
            return
          }
          setChecksState((current) =>
            updateGitHubChecksTabDetails(current, key, {
              loading: false,
              details: null,
              error: err instanceof Error ? err.message : 'Failed to load check details.'
            })
          )
        })
    },
    [detailsByCheckKey, mountedRef, prRepo, repoId, repoPath]
  )

  const refreshAction = (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          className="size-7 shrink-0"
          disabled={!repoPath || refreshing}
          onClick={() => void handleRefresh()}
          aria-label="Refresh checks"
        >
          <RefreshCw className={cn('size-3.5', refreshing && 'animate-spin')} />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={6}>
        Refresh checks
      </TooltipContent>
    </Tooltip>
  )
  const fixBrokenChecksAction =
    failedChecks.length > 0 || fixingChecks ? (
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            type="button"
            variant="outline"
            size="xs"
            className="h-7 gap-1 px-2 text-[11px]"
            disabled={!canFixBrokenChecks || fixingChecks}
            onClick={() => void handleFixBrokenChecks()}
          >
            {fixingChecks ? (
              <LoaderCircle className="size-3 animate-spin" />
            ) : (
              <Wrench className="size-3" />
            )}
            {variant === 'compact' ? 'Fix checks' : 'Fix broken checks'}
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={6}>
          Open New Spec, or resume the workspace already attached to this pull request
        </TooltipContent>
      </Tooltip>
    ) : null
  const rerunAction =
    list.length > 0 || rerunning ? (
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="outline"
            size="xs"
            className="h-7 gap-1 px-2 text-[11px]"
            disabled={!repoPath || rerunning || list.length === 0}
          >
            {rerunning ? (
              <LoaderCircle className="size-3 animate-spin" />
            ) : (
              <RefreshCw className="size-3" />
            )}
            Rerun
            <ChevronDown className="size-3 opacity-60" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-44">
          <DropdownMenuItem
            disabled={failedChecks.length === 0 || rerunning}
            onSelect={() => void handleRerun(true)}
          >
            <RefreshCw className="size-4" />
            Rerun failed checks
          </DropdownMenuItem>
          <DropdownMenuItem disabled={rerunning} onSelect={() => void handleRerun(false)}>
            <RefreshCw className="size-4" />
            Rerun all checks
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    ) : null
  const secondaryActions =
    variant === 'compact' && !fixBrokenChecksAction ? null : fixBrokenChecksAction ||
      rerunAction ? (
      <div className="flex min-w-0 flex-wrap items-center justify-end gap-1.5">
        {fixBrokenChecksAction}
        {variant === 'page' ? rerunAction : null}
      </div>
    ) : null
  const actions = (
    <div className="flex min-w-0 flex-wrap items-center justify-end gap-1.5">
      {refreshAction}
      {fixBrokenChecksAction}
      {rerunAction}
    </div>
  )
  const compactHeader = (
    <div className="border-b border-border/50 px-3 py-2">
      <div className="flex min-w-0 items-start gap-2">
        <div className="flex min-w-0 flex-1 items-start gap-2">
          <SummaryIcon
            className={cn(
              'mt-0.5 size-3.5 shrink-0',
              summaryColor,
              counts.pending > 0 && counts.failing === 0 && 'animate-spin'
            )}
          />
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-medium leading-5 text-foreground">Checks</div>
            {list.length > 0 && (
              <div className="truncate text-[11px] leading-4 text-muted-foreground">
                {summaryLabel}
              </div>
            )}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {refreshAction}
          {list.length > 0 && (
            <div className="[&_button]:h-7 [&_button]:px-2 [&_button]:text-[11px]">
              {rerunAction}
            </div>
          )}
        </div>
      </div>
      {secondaryActions ? (
        <div className="mt-2 flex min-w-0 justify-end">{secondaryActions}</div>
      ) : null}
    </div>
  )

  const renderCheckRow = (check: PRCheckDetail): React.JSX.Element => {
    const conclusion = getCheckConclusion(check)
    const Icon = CHECK_ICON[conclusion] ?? CircleDashed
    const color = CHECK_COLOR[conclusion] ?? 'text-muted-foreground'
    const statusLabel = getCheckStatusLabel(check)
    const key = getCheckDetailsKey(check)
    const expanded = expandedCheckKey === key
    const detailsState = detailsByCheckKey[key]
    return (
      <div key={key} className="min-w-0">
        <button
          type="button"
          onClick={() => handleToggleCheckDetails(check)}
          aria-expanded={expanded}
          className={cn(
            'flex w-full min-w-0 items-center gap-2 rounded-md text-left transition',
            variant === 'page' ? 'px-3 py-2.5 hover:bg-accent/60' : 'px-2 py-1.5 hover:bg-muted/40'
          )}
        >
          <ChevronDown
            className={cn(
              'size-3 shrink-0 text-muted-foreground transition-transform',
              !expanded && '-rotate-90'
            )}
          />
          <Icon
            className={cn('size-3.5 shrink-0', color, conclusion === 'pending' && 'animate-spin')}
          />
          <span className="min-w-0 flex-1 truncate text-[12px] text-foreground">{check.name}</span>
          <span className="shrink-0 text-[11px] text-muted-foreground">{statusLabel}</span>
        </button>
        {expanded && renderCheckDetails(check, detailsState)}
      </div>
    )
  }

  const renderCheckDetails = (
    check: PRCheckDetail,
    state: CheckDetailsLoadState | undefined
  ): React.JSX.Element => {
    const details = state?.details
    const openUrl = details?.detailsUrl ?? details?.url ?? check.url
    const startedAt = formatCheckTimestamp(details?.startedAt)
    const completedAt = formatCheckTimestamp(details?.completedAt)
    const detailsStatusCheck: PRCheckDetail = {
      ...check,
      status: (details?.status as PRCheckDetail['status'] | undefined) ?? check.status,
      conclusion:
        (details?.conclusion as PRCheckDetail['conclusion'] | undefined) ?? check.conclusion
    }
    const hasOutput = Boolean(details?.title || details?.summary || details?.text)
    const hasAnnotations = (details?.annotations.length ?? 0) > 0
    const hasJobs = (details?.jobs.length ?? 0) > 0

    return (
      <div className="mx-2 mb-2 mt-1 min-w-0 rounded-md border border-border/50 bg-muted/20 px-3 py-2">
        {state?.loading ? (
          <div className="flex items-center gap-2 py-2 text-[12px] text-muted-foreground">
            <LoaderCircle className="size-3.5 animate-spin" />
            Loading check details…
          </div>
        ) : (
          <div className="flex min-w-0 flex-col gap-2">
            <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
              <span>
                Status:{' '}
                {details ? getCheckStatusLabel(detailsStatusCheck) : getCheckStatusLabel(check)}
              </span>
              {startedAt && <span>Started {startedAt}</span>}
              {completedAt && <span>Completed {completedAt}</span>}
              {check.checkRunId && <span className="font-mono">check #{check.checkRunId}</span>}
            </div>

            {state?.error && <div className="text-[12px] text-muted-foreground">{state.error}</div>}

            {hasOutput && (
              <div className="min-w-0 rounded-md border border-border/40 bg-background/70 px-2.5 py-2">
                {details?.title && (
                  <div className="mb-1 text-[12px] font-medium text-foreground">
                    {details.title}
                  </div>
                )}
                {details?.summary && (
                  <CommentMarkdown
                    content={details.summary}
                    variant="document"
                    className="min-w-0 max-w-full overflow-hidden break-words text-[12px] leading-relaxed [&_a]:break-all [&_code]:break-words [&_pre]:max-w-full"
                  />
                )}
                {details?.text && (
                  <CommentMarkdown
                    content={details.text}
                    variant="document"
                    className="mt-2 min-w-0 max-w-full overflow-hidden break-words text-[12px] leading-relaxed [&_a]:break-all [&_code]:break-words [&_pre]:max-w-full"
                  />
                )}
              </div>
            )}

            {hasAnnotations && (
              <div className="min-w-0 rounded-md border border-border/40 bg-background/70">
                <div className="border-b border-border/40 px-2.5 py-1.5 text-[11px] font-medium text-foreground">
                  Annotations
                </div>
                <div className="flex max-h-48 flex-col overflow-y-auto scrollbar-sleek">
                  {details!.annotations.map((annotation, index) => (
                    <div
                      key={`${annotation.path ?? 'annotation'}-${index}`}
                      className={cn(
                        'min-w-0 px-2.5 py-2 text-[12px]',
                        index > 0 && 'border-t border-border/30'
                      )}
                    >
                      <div className="flex min-w-0 items-center gap-2">
                        <span className="min-w-0 truncate font-mono text-[11px] text-muted-foreground">
                          {annotation.path ?? 'Annotation'}
                          {annotation.startLine ? `:${annotation.startLine}` : ''}
                        </span>
                        {annotation.annotationLevel && (
                          <span className="shrink-0 text-[11px] text-muted-foreground">
                            {annotation.annotationLevel}
                          </span>
                        )}
                      </div>
                      {annotation.title && (
                        <div className="mt-1 text-[12px] font-medium text-foreground">
                          {annotation.title}
                        </div>
                      )}
                      <div className="mt-1 break-words text-[12px] text-foreground">
                        {annotation.message}
                      </div>
                      {annotation.rawDetails && (
                        <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap rounded bg-muted/40 p-2 font-mono text-[11px] text-muted-foreground scrollbar-sleek">
                          {annotation.rawDetails}
                        </pre>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            )}

            {hasJobs && (
              <div className="min-w-0 rounded-md border border-border/40 bg-background/70">
                <div className="border-b border-border/40 px-2.5 py-1.5 text-[11px] font-medium text-foreground">
                  Jobs
                </div>
                <div className="flex max-h-64 flex-col overflow-y-auto scrollbar-sleek">
                  {details!.jobs.map((job, index) => (
                    <div
                      key={`${job.name}-${index}`}
                      className={cn(
                        'min-w-0 px-2.5 py-2',
                        index > 0 && 'border-t border-border/30'
                      )}
                    >
                      <div className="flex min-w-0 items-center gap-2">
                        <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-foreground">
                          {job.name}
                        </span>
                        <span className="shrink-0 text-[11px] text-muted-foreground">
                          {job.conclusion ?? job.status ?? 'unknown'}
                        </span>
                      </div>
                      {job.steps.length > 0 && (
                        <div className="mt-1 grid gap-1">
                          {job.steps.map((step) => (
                            <div
                              key={step.name}
                              className="flex min-w-0 items-center gap-2 text-[11px] text-muted-foreground"
                            >
                              <span className="min-w-0 flex-1 truncate">{step.name}</span>
                              <span className="shrink-0">{step.conclusion ?? step.status}</span>
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            )}

            {!state?.error && !hasOutput && !hasAnnotations && !hasJobs && (
              <div className="text-[12px] text-muted-foreground">
                No inline output is available for this check.
              </div>
            )}

            {openUrl && (
              <div>
                <Button
                  type="button"
                  variant="ghost"
                  size="xs"
                  className="h-7 gap-1 px-2 text-[11px]"
                  onClick={() => api.shell.openUrl(openUrl)}
                >
                  Open in GitHub
                  <ExternalLink className="size-3" />
                </Button>
              </div>
            )}
          </div>
        )}
      </div>
    )
  }

  if (loading && list.length === 0) {
    return (
      <>
        {variant === 'compact' ? compactHeader : null}
        <div className="flex items-center justify-center py-10">
          <LoaderCircle className="size-5 animate-spin text-muted-foreground" />
        </div>
      </>
    )
  }
  if (list.length === 0) {
    if (variant === 'page') {
      return (
        <div className="flex flex-col gap-3 px-4 py-3">
          <div className="flex min-w-0 items-center gap-3">
            <CircleDashed className="size-4 shrink-0 text-muted-foreground" />
            <div className="flex min-w-0 flex-1 flex-col">
              <span className="truncate text-[13px] font-medium text-foreground">
                No checks found
              </span>
              <span className="truncate text-[11px] text-muted-foreground">
                This pull request has no reported checks yet.
              </span>
            </div>
            {actions}
          </div>
        </div>
      )
    }
    return (
      <>
        {compactHeader}
        <div className="flex flex-col items-center justify-center gap-1 px-4 py-6 text-center">
          <CircleDashed className="size-4 text-muted-foreground/60" />
          <div className="text-[12px] text-muted-foreground">No checks reported yet</div>
        </div>
      </>
    )
  }
  if (variant === 'page') {
    const countChips: { label: string; className: string }[] = []
    if (counts.passing > 0) {
      countChips.push({ label: `${counts.passing} passing`, className: CHECK_COLOR.success })
    }
    if (counts.failing > 0) {
      countChips.push({ label: `${counts.failing} failing`, className: CHECK_COLOR.failure })
    }
    if (counts.pending > 0) {
      countChips.push({ label: `${counts.pending} pending`, className: CHECK_COLOR.pending })
    }
    if (counts.skipped + counts.neutral > 0) {
      countChips.push({
        label: `${counts.skipped + counts.neutral} skipped`,
        className: 'text-muted-foreground'
      })
    }
    return (
      <div className="flex flex-col gap-3 px-4 py-3">
        <div className="flex min-w-0 items-center gap-3">
          <SummaryIcon
            className={cn(
              'size-4 shrink-0',
              summaryColor,
              counts.pending > 0 && counts.failing === 0 && 'animate-spin'
            )}
          />
          <div className="flex min-w-0 flex-1 items-center gap-2">
            <span className="truncate text-[13px] font-medium text-foreground">{summaryLabel}</span>
            {countChips.length > 1 && (
              <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                {countChips.map((chip, i) => (
                  <React.Fragment key={chip.label}>
                    {i > 0 && <span className="opacity-40">·</span>}
                    <span className={chip.className}>{chip.label}</span>
                  </React.Fragment>
                ))}
              </span>
            )}
          </div>
          {actions}
        </div>
        <div className="overflow-hidden rounded-lg border border-border/50 bg-card/50 shadow-xs">
          {sorted.map((check, index) => (
            <div
              key={getCheckDetailsKey(check)}
              className={cn(index > 0 && 'border-t border-border/40')}
            >
              {renderCheckRow(check)}
            </div>
          ))}
        </div>
      </div>
    )
  }
  return (
    <>
      {compactHeader}
      <div className="max-h-[280px] overflow-y-auto p-1 scrollbar-sleek">
        {sorted.map(renderCheckRow)}
      </div>
    </>
  )
}
