import { api } from '@/tauri'
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { AlertCircle, Check, CheckCircle2, ChevronDown, ChevronLeft, ChevronRight, CircleDot, Clock3, ExternalLink, GitMerge, LoaderCircle, Minus, Users } from 'lucide-react'
import { toast } from 'sonner'
import { useAppStore } from '@/store'
import { callRuntimeRpc, getActiveRuntimeTarget } from '@/runtime/runtime-rpc-client'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useConfirmationDialog } from '@/components/confirmation-dialog'
import { getGitHubPRPrimaryReviewer, getGitHubPRReviewLabel, normalizeGitHubReviewerLogins, type GitHubPRPrimaryReviewer } from '@/components/github-pr-reviewer-display'
import { getLinearStateMarkerStyle, getLinearStatePillStyle } from '@/components/linear-state-pill-style'
import { parseGitHubIssueOrPRLink } from '@/lib/github-links'
import { useRepoAssigneesBySlug } from '@/hooks/useGitHubSlugMetadata'
import { cn } from '@/lib/utils'
import { createTaskPageGitHubStatusStateDraft, resolveTaskPageGitHubStatusStateDraft, updateTaskPageGitHubStatusLocalState } from '@/components/task-page-github-status-state'
import { presentGitHubPRMergeState } from '@/components/github-pr-merge-state'
import { GITHUB_PR_MERGE_METHOD_LABELS, resolveGitHubPRMergeMethods } from '../../../shared/github-pr-merge-methods'
import type { GitHubAssignableUser, GitHubPRMergeMethod, GitHubWorkItem, LinearIssue, Repo } from '../../../shared/types'
import { useTeamStates } from '@/hooks/useIssueMetadata'
import { linearUpdateIssue } from '@/runtime/runtime-linear-client'
import { getChecksLabel, getChecksTone, getReviewTone } from './task-page/work-item-helpers'
import { buildRequestedReviewUsers, mergeReviewerSuggestions } from '@/lib/github-reviewers'

export function LinearStateCell({
  issue,
  className
}: {
  issue: LinearIssue
  className?: string
}): React.JSX.Element {
  const settings = useAppStore((s) => s.settings)
  const patchLinearIssue = useAppStore((s) => s.patchLinearIssue)
  const states = useTeamStates(issue.team.id, settings, issue.workspaceId)
  const [open, setOpen] = useState(false)
  const [pending, setPending] = useState(false)
  const reqRef = useRef(0)

  const currentStateId = states.data.find(
    (s) => s.name === issue.state.name && s.type === issue.state.type
  )?.id

  const handleStateChange = useCallback(
    (stateId: string) => {
      const newState = states.data.find((s) => s.id === stateId)
      if (!newState || stateId === currentStateId || pending) {
        return
      }

      reqRef.current += 1
      const reqId = reqRef.current
      const previousState = issue.state
      const nextState: LinearIssue['state'] = {
        name: newState.name,
        type: newState.type,
        color: newState.color
      }

      setPending(true)
      patchLinearIssue(issue.id, { state: nextState })
      void linearUpdateIssue(settings, issue.id, { stateId }, issue.workspaceId)
        .then((result) => {
          if (reqId !== reqRef.current) {
            return
          }
          if (result.ok === false) {
            patchLinearIssue(issue.id, { state: previousState })
            toast.error(result.error ?? 'Failed to update Linear state')
          }
        })
        .catch(() => {
          if (reqId !== reqRef.current) {
            return
          }
          patchLinearIssue(issue.id, { state: previousState })
          toast.error('Failed to update Linear state')
        })
        .finally(() => {
          if (reqId === reqRef.current) {
            setPending(false)
          }
        })
    },
    [
      currentStateId,
      issue.id,
      issue.state,
      issue.workspaceId,
      patchLinearIssue,
      pending,
      settings,
      states.data
    ]
  )

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={pending}
          onClick={(e) => e.stopPropagation()}
          className={cn(
            'inline-flex min-w-0 cursor-pointer! items-center gap-1 rounded-full border text-[11px] font-medium transition-[background-color,border-color,color,box-shadow] hover:[--linear-state-pill-current-background:var(--linear-state-pill-hover-background)] hover:[--linear-state-pill-current-border:var(--linear-state-pill-hover-border)] hover:[--linear-state-pill-current-foreground:var(--linear-state-pill-hover-foreground)] hover:ring-1 hover:ring-foreground/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:cursor-default! disabled:opacity-80 [&_*]:cursor-pointer! disabled:[&_*]:cursor-default!',
            className
          )}
          style={{
            ...getLinearStatePillStyle(issue.state.color),
            cursor: pending ? 'default' : 'pointer'
          }}
          aria-label={`Change Linear state from ${issue.state.name}`}
          aria-busy={pending || states.loading}
        >
          <span
            className="size-1.5 shrink-0 rounded-full"
            style={getLinearStateMarkerStyle(issue.state.color)}
          />
          <span className="truncate">{issue.state.name}</span>
          {pending || states.loading ? (
            <LoaderCircle className="size-3 shrink-0 animate-spin opacity-70" />
          ) : (
            <ChevronDown className="size-3 shrink-0 opacity-55" />
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="popover-scroll-content scrollbar-sleek w-48 p-1"
        align="start"
        onClick={(e) => e.stopPropagation()}
      >
        {states.error ? (
          <div className="px-2 py-3 text-center text-[12px] text-destructive">{states.error}</div>
        ) : states.loading ? (
          <div className="flex items-center gap-2 px-2 py-3 text-[12px] text-muted-foreground">
            <LoaderCircle className="size-3 animate-spin" />
            Loading states
          </div>
        ) : states.data.length > 0 ? (
          states.data.map((state) => (
            <button
              key={state.id}
              type="button"
              onClick={() => {
                handleStateChange(state.id)
                setOpen(false)
              }}
              className={cn(
                'flex w-full cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-left text-[12px] hover:bg-accent',
                currentStateId === state.id && 'bg-accent/50'
              )}
            >
              <span
                className="inline-block size-2 rounded-full"
                style={{ backgroundColor: state.color }}
              />
              {state.name}
            </button>
          ))
        ) : (
          <div className="px-2 py-3 text-center text-[12px] text-muted-foreground">
            No states found
          </div>
        )}
      </PopoverContent>
    </Popover>
  )
}

export function GHStatusCell({
  item,
  repo
}: {
  item: GitHubWorkItem
  repo: Repo | null
}): React.JSX.Element {
  const patchWorkItem = useAppStore((s) => s.patchWorkItem)
  const [statusStateDraft, setStatusStateDraft] = useState(() =>
    createTaskPageGitHubStatusStateDraft(item)
  )
  const [open, setOpen] = useState(false)
  const reqRef = useRef(0)

  const resolvedStatusStateDraft = resolveTaskPageGitHubStatusStateDraft(statusStateDraft, item)
  if (resolvedStatusStateDraft !== statusStateDraft) {
    // Why: item rows can refresh from the GitHub cache while this cell is still
    // mounted; reconcile before paint instead of showing one stale status frame.
    setStatusStateDraft(resolvedStatusStateDraft)
  }
  const localState = resolvedStatusStateDraft.localState
  const updateLocalState = useCallback(
    (nextState: GitHubWorkItem['state']) => {
      setStatusStateDraft((current) =>
        updateTaskPageGitHubStatusLocalState(current, item, nextState)
      )
    },
    [item]
  )

  const handleStateChange = useCallback(
    (newState: 'open' | 'closed') => {
      if (newState === localState || !repo || item.type !== 'issue') {
        return
      }
      reqRef.current += 1
      const reqId = reqRef.current
      updateLocalState(newState)
      patchWorkItem(item.id, { state: newState }, item.repoId)
      const target = getActiveRuntimeTarget(useAppStore.getState().settings)
      const updatePromise =
        target.kind === 'environment'
          ? callRuntimeRpc<{ ok?: boolean; error?: string }>(
              target,
              'github.updateIssue',
              { repo: repo.id, number: item.number, updates: { state: newState } },
              { timeoutMs: 30_000 }
            )
          : api.gh.updateIssue({
              repoPath: repo.path,
              repoId: repo.id,
              number: item.number,
              updates: { state: newState }
            })
      updatePromise
        .then((result) => {
          if (reqId !== reqRef.current) {
            return
          }
          const typed = result as { ok?: boolean; error?: string }
          if (typed && typed.ok === false) {
            updateLocalState(newState === 'closed' ? 'open' : 'closed')
            patchWorkItem(
              item.id,
              { state: newState === 'closed' ? 'open' : 'closed' },
              item.repoId
            )
            toast.error(typed.error ?? 'Failed to update state')
          }
        })
        .catch(() => {
          if (reqId !== reqRef.current) {
            return
          }
          updateLocalState(newState === 'closed' ? 'open' : 'closed')
          patchWorkItem(item.id, { state: newState === 'closed' ? 'open' : 'closed' }, item.repoId)
          toast.error('Failed to update state')
        })
    },
    [item, localState, repo, patchWorkItem, updateLocalState]
  )

  if (item.type !== 'issue' || !repo) {
    return (
      <span className="rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2 py-0.5 text-[10px] font-medium text-emerald-700 opacity-70 dark:text-emerald-200">
        Open
      </span>
    )
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          onClick={(e) => e.stopPropagation()}
          className={cn(
            'group/status inline-flex cursor-pointer items-center gap-0.5 rounded-full border px-2 py-0.5 text-[10px] font-medium transition hover:brightness-125 hover:ring-1 hover:ring-white/10',
            localState === 'closed'
              ? 'border-rose-500/30 bg-rose-500/10 text-rose-600 dark:text-rose-300'
              : 'border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-300'
          )}
        >
          {localState === 'closed' ? 'Closed' : 'Open'}
          <ChevronDown className="size-2.5 opacity-50" />
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-36 p-1" align="start" onClick={(e) => e.stopPropagation()}>
        <button
          type="button"
          onClick={() => {
            handleStateChange('open')
            setOpen(false)
          }}
          className={cn(
            'flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-[12px] hover:bg-accent',
            localState === 'open' && 'bg-accent/50'
          )}
        >
          <CircleDot className="size-3 text-emerald-500" />
          Open
        </button>
        <button
          type="button"
          onClick={() => {
            handleStateChange('closed')
            setOpen(false)
          }}
          className={cn(
            'flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-[12px] hover:bg-accent',
            localState === 'closed' && 'bg-accent/50'
          )}
        >
          <CircleDot className="size-3 text-rose-500" />
          Closed
        </button>
      </PopoverContent>
    </Popover>
  )
}

function ReviewChipAvatar({
  reviewer
}: {
  reviewer: GitHubPRPrimaryReviewer | null
}): React.JSX.Element {
  if (reviewer?.avatarUrl) {
    return (
      <img
        src={reviewer.avatarUrl}
        alt=""
        loading="lazy"
        decoding="async"
        title={reviewer.name ? `${reviewer.name} (${reviewer.login})` : reviewer.login}
        className="size-3.5 shrink-0 rounded-full border border-border/50 bg-muted object-cover"
      />
    )
  }
  if (reviewer?.login) {
    return (
      <span
        title={reviewer.login}
        className="inline-flex size-3.5 shrink-0 items-center justify-center rounded-full border border-border/50 bg-muted text-[8px] font-medium text-muted-foreground"
      >
        {reviewer.login.slice(0, 1).toUpperCase()}
      </span>
    )
  }
  return <Users className="size-3 shrink-0" />
}

function GitHubAssigneeAvatar({ assignee }: { assignee: GitHubAssignableUser }): React.JSX.Element {
  if (assignee.avatarUrl) {
    return (
      <img
        src={assignee.avatarUrl}
        alt={assignee.login}
        loading="lazy"
        decoding="async"
        title={assignee.name ? `${assignee.name} (${assignee.login})` : assignee.login}
        className="size-5 rounded-full border border-border/40 bg-muted object-cover"
      />
    )
  }
  return (
    <span
      title={assignee.login}
      className="inline-flex size-5 items-center justify-center rounded-full border border-border/40 bg-muted text-[10px] font-medium text-muted-foreground"
    >
      {assignee.login.slice(0, 1).toUpperCase()}
    </span>
  )
}

export function GitHubIssueLabelSelector({
  labels,
  selectedLabels,
  loading,
  error,
  disabled,
  onChange
}: {
  labels: string[]
  selectedLabels: string[]
  loading: boolean
  error: string | null
  disabled: boolean
  onChange: (labels: string[]) => void
}): React.JSX.Element {
  const selectedSet = useMemo(() => new Set(selectedLabels), [selectedLabels])
  const toggleLabel = useCallback(
    (label: string) => {
      onChange(
        selectedSet.has(label)
          ? selectedLabels.filter((name) => name !== label)
          : [...selectedLabels, label]
      )
    },
    [onChange, selectedLabels, selectedSet]
  )

  return (
    <div className="flex min-w-0 flex-col gap-1">
      <label className="text-[11px] font-medium text-muted-foreground">Labels</label>
      <Popover>
        <PopoverTrigger asChild>
          <Button
            type="button"
            variant="outline"
            disabled={disabled}
            className="h-auto min-h-9 justify-start gap-2 px-3 py-2 text-left"
          >
            {selectedLabels.length === 0 ? (
              <span className="text-muted-foreground">None</span>
            ) : (
              <span className="flex min-w-0 flex-wrap gap-1.5">
                {selectedLabels.map((label) => (
                  <span
                    key={label}
                    className="rounded-full border border-border/50 bg-muted/40 px-2 py-0.5 text-[11px] font-medium"
                  >
                    {label}
                  </span>
                ))}
              </span>
            )}
            {loading ? <LoaderCircle className="ml-auto size-3.5 animate-spin" /> : null}
          </Button>
        </PopoverTrigger>
        <PopoverContent className="popover-scroll-content scrollbar-sleek w-64 p-1" align="start">
          {error ? (
            <div className="px-2 py-2 text-xs text-destructive">{error}</div>
          ) : labels.length === 0 ? (
            <div className="px-2 py-2 text-xs text-muted-foreground">No labels.</div>
          ) : (
            labels.map((label) => (
              <button
                key={label}
                type="button"
                onClick={() => toggleLabel(label)}
                className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs hover:bg-accent"
              >
                <span
                  className={cn(
                    'flex size-3.5 shrink-0 items-center justify-center rounded-sm border',
                    selectedSet.has(label)
                      ? 'border-primary bg-primary text-primary-foreground'
                      : 'border-input'
                  )}
                >
                  {selectedSet.has(label) ? <Check className="size-2.5" /> : null}
                </span>
                <span className="min-w-0 truncate">{label}</span>
              </button>
            ))
          )}
        </PopoverContent>
      </Popover>
    </div>
  )
}

export function GitHubIssueAssigneeSelector({
  assignees,
  selectedAssignees,
  loading,
  error,
  disabled,
  onChange
}: {
  assignees: GitHubAssignableUser[]
  selectedAssignees: GitHubAssignableUser[]
  loading: boolean
  error: string | null
  disabled: boolean
  onChange: (assignees: GitHubAssignableUser[]) => void
}): React.JSX.Element {
  const selectedLogins = useMemo(
    () => new Set(selectedAssignees.map((assignee) => assignee.login.toLowerCase())),
    [selectedAssignees]
  )
  const toggleAssignee = useCallback(
    (assignee: GitHubAssignableUser) => {
      const key = assignee.login.toLowerCase()
      onChange(
        selectedLogins.has(key)
          ? selectedAssignees.filter((current) => current.login.toLowerCase() !== key)
          : [...selectedAssignees, assignee]
      )
    },
    [onChange, selectedAssignees, selectedLogins]
  )

  return (
    <div className="flex min-w-0 flex-col gap-1">
      <label className="text-[11px] font-medium text-muted-foreground">Assignees</label>
      <Popover>
        <PopoverTrigger asChild>
          <Button
            type="button"
            variant="outline"
            disabled={disabled}
            className="h-auto min-h-9 justify-start gap-2 px-3 py-2 text-left"
          >
            {selectedAssignees.length === 0 ? (
              <span className="text-muted-foreground">Unassigned</span>
            ) : (
              <span className="flex min-w-0 items-center gap-1.5">
                <span className="flex -space-x-1">
                  {selectedAssignees.slice(0, 3).map((assignee) => (
                    <GitHubAssigneeAvatar key={assignee.login} assignee={assignee} />
                  ))}
                </span>
                <span className="min-w-0 truncate text-xs">
                  {selectedAssignees.map((assignee) => assignee.login).join(', ')}
                </span>
              </span>
            )}
            {loading ? <LoaderCircle className="ml-auto size-3.5 animate-spin" /> : null}
          </Button>
        </PopoverTrigger>
        <PopoverContent className="popover-scroll-content scrollbar-sleek w-72 p-1" align="start">
          {error ? (
            <div className="px-2 py-2 text-xs text-destructive">{error}</div>
          ) : assignees.length === 0 ? (
            <div className="px-2 py-2 text-xs text-muted-foreground">No assignable users.</div>
          ) : (
            assignees.map((assignee) => {
              const selected = selectedLogins.has(assignee.login.toLowerCase())
              return (
                <button
                  key={assignee.login}
                  type="button"
                  onClick={() => toggleAssignee(assignee)}
                  className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs hover:bg-accent"
                >
                  <span
                    className={cn(
                      'flex size-3.5 shrink-0 items-center justify-center rounded-sm border',
                      selected
                        ? 'border-primary bg-primary text-primary-foreground'
                        : 'border-input'
                    )}
                  >
                    {selected ? <Check className="size-2.5" /> : null}
                  </span>
                  <GitHubAssigneeAvatar assignee={assignee} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-medium">{assignee.login}</span>
                    {assignee.name ? (
                      <span className="block truncate text-[11px] text-muted-foreground">
                        {assignee.name}
                      </span>
                    ) : null}
                  </span>
                </button>
              )
            })
          )}
        </PopoverContent>
      </Popover>
    </div>
  )
}

export function GHAssigneesCell({
  item,
  repo
}: {
  item: GitHubWorkItem
  repo: Repo | null
}): React.JSX.Element {
  const patchWorkItem = useAppStore((s) => s.patchWorkItem)
  const settings = useAppStore((s) => s.settings)
  const [open, setOpen] = useState(false)
  const [pendingLogin, setPendingLogin] = useState<string | null>(null)
  const assignees = useMemo(() => item.assignees ?? [], [item.assignees])
  const parsed = useMemo(() => parseGitHubIssueOrPRLink(item.url), [item.url])
  const owner = parsed?.slug.owner ?? null
  const repoName = parsed?.slug.repo ?? null
  const seedLogins = useMemo(
    () =>
      assignees
        .map((a) => a.login)
        .sort()
        .filter(Boolean),
    [assignees]
  )
  const metadata = useRepoAssigneesBySlug(
    open ? owner : null,
    open ? repoName : null,
    seedLogins,
    settings
  )

  const toggleAssignee = useCallback(
    async (user: GitHubAssignableUser): Promise<void> => {
      if (item.type !== 'issue' || pendingLogin) {
        return
      }
      const userLoginKey = user.login.toLowerCase()
      const isOn = assignees.some((a) => a.login.toLowerCase() === userLoginKey)
      const previousAssignees = assignees
      const nextAssignees = isOn
        ? assignees.filter((a) => a.login.toLowerCase() !== userLoginKey)
        : [...assignees, user]
      setPendingLogin(user.login)
      patchWorkItem(item.id, { assignees: nextAssignees }, item.repoId)

      try {
        const updates = isOn ? { removeAssignees: [user.login] } : { addAssignees: [user.login] }
        const target = getActiveRuntimeTarget(settings)
        if (owner && repoName) {
          const args = {
            owner,
            repo: repoName,
            number: item.number,
            updates
          }
          const res =
            target.kind === 'environment'
              ? await callRuntimeRpc<Awaited<ReturnType<typeof api.gh.updateIssueBySlug>>>(
                  target,
                  'github.project.updateIssueBySlug',
                  args,
                  { timeoutMs: 30_000 }
                )
              : await api.gh.updateIssueBySlug(args)
          if (!res.ok) {
            throw new Error(res.error.message)
          }
        } else if (repo) {
          const res =
            target.kind === 'environment'
              ? await callRuntimeRpc<{ ok?: boolean; error?: string }>(
                  target,
                  'github.updateIssue',
                  { repo: repo.id, number: item.number, updates },
                  { timeoutMs: 30_000 }
                )
              : await api.gh.updateIssue({
                  repoPath: repo.path,
                  repoId: repo.id,
                  number: item.number,
                  updates
                })
          if (res && res.ok === false) {
            throw new Error(res.error)
          }
        } else {
          throw new Error('No GitHub repository context available for this issue.')
        }
      } catch (err) {
        patchWorkItem(item.id, { assignees: previousAssignees }, item.repoId)
        toast.error(err instanceof Error ? err.message : 'Failed to update assignees.')
      } finally {
        setPendingLogin(null)
      }
    },
    [
      assignees,
      item.id,
      item.number,
      item.repoId,
      item.type,
      owner,
      patchWorkItem,
      pendingLogin,
      repo,
      repoName,
      settings
    ]
  )

  const triggerContent =
    assignees.length > 0 ? (
      <>
        <div className="flex min-w-0 -space-x-1 overflow-hidden">
          {assignees.slice(0, 3).map((assignee) => (
            <GitHubAssigneeAvatar key={assignee.login} assignee={assignee} />
          ))}
        </div>
        {assignees.length > 3 ? (
          <span className="ml-1 shrink-0 text-[10px] font-medium text-muted-foreground">
            +{assignees.length - 3}
          </span>
        ) : null}
      </>
    ) : (
      <span className="text-xs text-muted-foreground/60">-</span>
    )

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={
            assignees.length
              ? `Assigned to ${assignees.map((a) => a.login).join(', ')}`
              : 'Assign issue'
          }
          aria-busy={pendingLogin !== null}
          onClick={(event) => event.stopPropagation()}
          onKeyDown={(event) => event.stopPropagation()}
          className={cn(
            'inline-flex h-6 max-w-full items-center gap-1 text-left transition disabled:opacity-60',
            assignees.length > 0
              ? 'rounded-full border border-border/40 bg-background/70 px-1.5 hover:bg-muted/60'
              : 'w-full rounded-sm border border-transparent bg-transparent px-1 hover:bg-muted/40'
          )}
        >
          {triggerContent}
          {pendingLogin ? (
            <LoaderCircle className="size-3 shrink-0 animate-spin text-muted-foreground" />
          ) : assignees.length > 0 ? (
            <ChevronDown className="size-3 shrink-0 text-muted-foreground" />
          ) : null}
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="popover-scroll-content scrollbar-sleek w-64 p-1"
        onClick={(event) => event.stopPropagation()}
      >
        {!owner || !repoName ? (
          <div className="px-2 py-2 text-xs text-muted-foreground">Issue has no repo slug.</div>
        ) : metadata.loading ? (
          <div className="px-2 py-2 text-xs text-muted-foreground">Loading…</div>
        ) : metadata.error ? (
          <div className="px-2 py-2 text-xs text-destructive">{metadata.error}</div>
        ) : metadata.data.length === 0 ? (
          <div className="px-2 py-2 text-xs text-muted-foreground">No assignable users.</div>
        ) : (
          metadata.data.map((user) => {
            const isOn = assignees.some((a) => a.login.toLowerCase() === user.login.toLowerCase())
            const pending = pendingLogin === user.login
            return (
              <button
                key={user.login}
                type="button"
                disabled={pendingLogin !== null}
                className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs hover:bg-muted/50 disabled:opacity-60"
                onClick={(event) => {
                  event.stopPropagation()
                  void toggleAssignee(user)
                }}
              >
                <span
                  className={cn(
                    'flex size-3.5 shrink-0 items-center justify-center rounded-sm border',
                    isOn ? 'border-primary bg-primary text-primary-foreground' : 'border-input'
                  )}
                >
                  {pending ? (
                    <LoaderCircle className="size-3 animate-spin" />
                  ) : isOn ? (
                    <Check className="size-3" />
                  ) : null}
                </span>
                {user.avatarUrl ? (
                  <img src={user.avatarUrl} alt="" className="size-5 shrink-0 rounded-full" />
                ) : (
                  <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-muted text-[10px] font-medium text-muted-foreground">
                    {user.login.slice(0, 1).toUpperCase()}
                  </span>
                )}
                <span className="min-w-0 flex-1">
                  <span className="block truncate">{user.login}</span>
                  {user.name ? (
                    <span className="block truncate text-[11px] text-muted-foreground">
                      {user.name}
                    </span>
                  ) : null}
                </span>
              </button>
            )
          })
        )}
      </PopoverContent>
    </Popover>
  )
}

export function PRReviewCell({
  item,
  repo
}: {
  item: GitHubWorkItem
  repo: Repo | null
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [reviewerInput, setReviewerInput] = useState('')
  const [localReviewRequests, setLocalReviewRequests] = useState<GitHubAssignableUser[]>(
    () => item.reviewRequests ?? []
  )
  const [reviewRequestsSource, setReviewRequestsSource] = useState(() => ({
    itemId: item.id,
    repoId: item.repoId,
    reviewRequests: item.reviewRequests
  }))
  const patchWorkItem = useAppStore((s) => s.patchWorkItem)
  const [activeReviewerCursor, setActiveReviewerCursor] = useState({ resetKey: '', index: 0 })
  const [submitting, setSubmitting] = useState(false)
  const settings = useAppStore((s) => s.settings)
  const reviewerInputRef = useRef<HTMLInputElement | null>(null)
  const reviewerInputFocusFrameRef = useRef<number | null>(null)

  const cancelReviewerInputFocusFrame = useCallback((): void => {
    if (reviewerInputFocusFrameRef.current === null) {
      return
    }
    cancelAnimationFrame(reviewerInputFocusFrameRef.current)
    reviewerInputFocusFrameRef.current = null
  }, [])

  const setReviewerInputNode = useCallback(
    (node: HTMLInputElement | null): void => {
      // Why: the queued picker focus is only valid while this input is mounted.
      if (!node) {
        cancelReviewerInputFocusFrame()
      }
      reviewerInputRef.current = node
    },
    [cancelReviewerInputFocusFrame]
  )

  // Why: reviewer edits are optimistic, but item switches/refetches must clear
  // stale local requests before paint; a passive Effect leaves one stale render.
  if (
    reviewRequestsSource.itemId !== item.id ||
    reviewRequestsSource.repoId !== item.repoId ||
    reviewRequestsSource.reviewRequests !== item.reviewRequests
  ) {
    setReviewRequestsSource({
      itemId: item.id,
      repoId: item.repoId,
      reviewRequests: item.reviewRequests
    })
    setLocalReviewRequests(item.reviewRequests ?? [])
  }

  const reviewerSeedUsers = useMemo<GitHubAssignableUser[]>(() => {
    const byLogin = new Map<string, GitHubAssignableUser>()
    const add = (user: GitHubAssignableUser): void => {
      if (!user.login) {
        return
      }
      byLogin.set(user.login.toLowerCase(), user)
    }
    for (const user of localReviewRequests) {
      add(user)
    }
    for (const review of item.latestReviews ?? []) {
      add({
        login: review.login,
        name: null,
        avatarUrl: review.avatarUrl ?? ''
      })
    }
    if (item.author) {
      add({ login: item.author, name: null, avatarUrl: '' })
    }
    return Array.from(byLogin.values())
  }, [item.author, item.latestReviews, localReviewRequests])

  const reviewSlug = useMemo(() => parseGitHubIssueOrPRLink(item.url)?.slug ?? null, [item.url])
  const reviewerMetadata = useRepoAssigneesBySlug(
    open && reviewSlug ? reviewSlug.owner : null,
    open && reviewSlug ? reviewSlug.repo : null,
    reviewerSeedUsers.map((user) => user.login),
    settings
  )

  const authorLogin = item.author?.toLowerCase() ?? null
  const reviewerCandidates = useMemo(
    () =>
      mergeReviewerSuggestions(reviewerMetadata.data, reviewerSeedUsers).filter(
        (user) => user.login.toLowerCase() !== authorLogin
      ),
    [authorLogin, reviewerMetadata.data, reviewerSeedUsers]
  )
  const reviewerCandidatesByLogin = useMemo(
    () => new Map(reviewerCandidates.map((user) => [user.login.toLowerCase(), user])),
    [reviewerCandidates]
  )
  const selectedReviewerLogins = useMemo(
    () =>
      new Set(
        localReviewRequests.map((reviewer) => reviewer.login.trim().toLowerCase()).filter(Boolean)
      ),
    [localReviewRequests]
  )
  const reviewerQuery = reviewerInput.trim().replace(/^@/, '').toLowerCase()
  const filteredReviewerCandidates = useMemo(() => {
    const query = reviewerQuery
    return reviewerCandidates
      .filter((user) => {
        const login = user.login.toLowerCase()
        return (
          query.length === 0 ||
          login.includes(query) ||
          (user.name ?? '').toLowerCase().includes(query)
        )
      })
      .sort((a, b) => {
        const aLogin = a.login.toLowerCase()
        const bLogin = b.login.toLowerCase()
        const aStarts = aLogin.startsWith(query)
        const bStarts = bLogin.startsWith(query)
        if (aStarts !== bStarts) {
          return aStarts ? -1 : 1
        }
        return a.login.localeCompare(b.login)
      })
  }, [reviewerCandidates, reviewerQuery])
  const suggestedReviewerRows = useMemo(
    () =>
      reviewerQuery.length === 0
        ? reviewerSeedUsers
            .filter((user) => !selectedReviewerLogins.has(user.login.toLowerCase()))
            .filter((user) => user.login.toLowerCase() !== authorLogin)
            .map((user) => reviewerCandidatesByLogin.get(user.login.toLowerCase()) ?? user)
            .slice(0, 1)
        : [],
    [
      authorLogin,
      reviewerCandidatesByLogin,
      reviewerQuery.length,
      reviewerSeedUsers,
      selectedReviewerLogins
    ]
  )
  const everyoneElseReviewerRows = useMemo(() => {
    const suggestedLogins = new Set(suggestedReviewerRows.map((user) => user.login.toLowerCase()))
    return filteredReviewerCandidates.filter(
      (user) => !suggestedLogins.has(user.login.toLowerCase())
    )
  }, [filteredReviewerCandidates, suggestedReviewerRows])
  const actionableReviewerRows = useMemo(
    () => [...suggestedReviewerRows, ...everyoneElseReviewerRows],
    [everyoneElseReviewerRows, suggestedReviewerRows]
  )

  const reviewerCursorResetKey = `${reviewerQuery}\u0000${actionableReviewerRows.length}`
  if (activeReviewerCursor.resetKey !== reviewerCursorResetKey) {
    setActiveReviewerCursor({ resetKey: reviewerCursorResetKey, index: 0 })
  }
  const activeReviewerIndex =
    activeReviewerCursor.resetKey === reviewerCursorResetKey ? activeReviewerCursor.index : 0
  const setActiveReviewerIndex = useCallback(
    (nextIndex: number | ((current: number) => number)): void => {
      setActiveReviewerCursor((current) => {
        const currentIndex = current.resetKey === reviewerCursorResetKey ? current.index : 0
        return {
          resetKey: reviewerCursorResetKey,
          index: typeof nextIndex === 'function' ? nextIndex(currentIndex) : nextIndex
        }
      })
    },
    [reviewerCursorResetKey]
  )

  if (item.type !== 'pr') {
    return <span className="text-[11px] text-muted-foreground">Issue</span>
  }

  const itemWithLocalReviewRequests = { ...item, reviewRequests: localReviewRequests }
  const primaryReviewer = getGitHubPRPrimaryReviewer(itemWithLocalReviewRequests)
  const hasReviewerMetadata =
    item.reviewDecision !== undefined ||
    localReviewRequests.length > 0 ||
    item.reviewRequests !== undefined ||
    item.latestReviews !== undefined

  const handleRequestReview = async (requestedLogins?: string[]): Promise<void> => {
    if (!repo || submitting) {
      return
    }
    const logins = normalizeGitHubReviewerLogins(
      requestedLogins ?? reviewerInput.split(/[\s,]+/),
      selectedReviewerLogins
    )
    if (logins.length === 0) {
      toast.error('Enter a reviewer')
      return
    }
    if (localReviewRequests.length + logins.length > 15) {
      toast.error('You can request up to 15 reviewers')
      return
    }
    setSubmitting(true)
    try {
      const target = getActiveRuntimeTarget(settings)
      const result =
        target.kind === 'environment'
          ? await callRuntimeRpc<{ ok: boolean; error?: string }>(
              target,
              'github.requestPRReviewers',
              { repo: repo.id, prNumber: item.number, reviewers: logins },
              { timeoutMs: 30_000 }
            )
          : await api.gh.requestPRReviewers({
              repoPath: repo.path,
              repoId: repo.id,
              prNumber: item.number,
              reviewers: logins
            })
      if (result.ok) {
        toast.success('Reviewer requested')
        const nextReviewRequests = buildRequestedReviewUsers(
          logins,
          reviewerCandidates,
          localReviewRequests
        )
        setLocalReviewRequests(nextReviewRequests)
        patchWorkItem(item.id, { reviewRequests: nextReviewRequests }, item.repoId)
        setReviewerInput('')
      } else {
        toast.error(result.error)
      }
    } catch {
      toast.error('Failed to request reviewer')
    } finally {
      setSubmitting(false)
    }
  }

  const requestReviewer = async (reviewer: GitHubAssignableUser): Promise<void> => {
    if (selectedReviewerLogins.has(reviewer.login.toLowerCase())) {
      return
    }
    // Close the popover immediately so the UI feels responsive; the GitHub
    // request runs in the background and toasts on completion.
    setOpen(false)
    setReviewerInput('')
    await handleRequestReview([reviewer.login])
  }

  const handleReviewerPickerOpenChange = (nextOpen: boolean): void => {
    setOpen(nextOpen)
    if (nextOpen) {
      cancelReviewerInputFocusFrame()
      reviewerInputFocusFrameRef.current = requestAnimationFrame(() => {
        reviewerInputFocusFrameRef.current = null
        reviewerInputRef.current?.focus()
      })
      return
    }
    cancelReviewerInputFocusFrame()
    setReviewerInput('')
  }

  const renderReviewerPickerRow = (
    reviewer: GitHubAssignableUser,
    options: { suggested: boolean; activeIndex: number }
  ): React.JSX.Element => {
    const selected = selectedReviewerLogins.has(reviewer.login.toLowerCase())
    const active = actionableReviewerRows[activeReviewerIndex]?.login === reviewer.login
    return (
      <button
        key={`${options.suggested ? 'suggested' : 'reviewer'}:${reviewer.login}`}
        type="button"
        className={cn(
          'flex min-h-10 w-full items-center gap-2 border-b border-border/50 px-3 py-2 text-left text-[13px] outline-none last:border-b-0 hover:bg-accent/70',
          active && 'bg-accent text-accent-foreground',
          selected && 'font-medium'
        )}
        onMouseEnter={() => setActiveReviewerIndex(options.activeIndex)}
        onMouseDown={(event) => {
          event.preventDefault()
          void requestReviewer(reviewer)
        }}
      >
        <span className="flex size-4 shrink-0 items-center justify-center text-foreground">
          {selected ? <Check className="size-3.5" /> : null}
        </span>
        {reviewer.avatarUrl ? (
          <img src={reviewer.avatarUrl} alt="" className="size-5 shrink-0 rounded-full" />
        ) : (
          <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-muted text-[10px] font-medium text-muted-foreground">
            {reviewer.login.slice(0, 1).toUpperCase()}
          </span>
        )}
        <span className="min-w-0 flex-1">
          <span className="block truncate">
            <span className="font-semibold text-foreground">{reviewer.login}</span>
            {reviewer.name ? (
              <span className="ml-1 font-normal text-muted-foreground">{reviewer.name}</span>
            ) : null}
          </span>
          {options.suggested ? (
            <span className="block truncate text-[12px] leading-4 text-muted-foreground">
              Recently active in this pull request
            </span>
          ) : null}
        </span>
      </button>
    )
  }

  return (
    <Popover open={open} onOpenChange={handleReviewerPickerOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          onClick={(event) => event.stopPropagation()}
          className={cn(
            'inline-flex max-w-full items-center gap-1 rounded-full border px-2 py-0.5 text-[10px] font-medium transition hover:brightness-110',
            getReviewTone(itemWithLocalReviewRequests)
          )}
        >
          <ReviewChipAvatar reviewer={primaryReviewer} />
          <span className="truncate">{getGitHubPRReviewLabel(itemWithLocalReviewRequests)}</span>
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[330px] overflow-hidden rounded-md border-border/70 p-0"
        align="start"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="border-b border-border/70 px-3 py-2">
          <div className="text-[13px] font-semibold text-foreground">
            Request up to 15 reviewers
          </div>
        </div>
        <div className="border-b border-border/70 p-3">
          <Input
            ref={setReviewerInputNode}
            value={reviewerInput}
            onChange={(event) => setReviewerInput(event.target.value)}
            placeholder="Type or choose a user"
            disabled={!repo || submitting}
            className="h-8 rounded-md bg-background px-2 text-[13px]"
            aria-label="Type or choose a user"
            aria-autocomplete="list"
            onKeyDown={(event) => {
              if (event.key === 'ArrowDown' && actionableReviewerRows.length > 0) {
                event.preventDefault()
                setActiveReviewerIndex((current) => (current + 1) % actionableReviewerRows.length)
                return
              }
              if (event.key === 'ArrowUp' && actionableReviewerRows.length > 0) {
                event.preventDefault()
                setActiveReviewerIndex(
                  (current) =>
                    (current - 1 + actionableReviewerRows.length) % actionableReviewerRows.length
                )
                return
              }
              if (event.key === 'Enter') {
                event.preventDefault()
                const activeReviewer = actionableReviewerRows[activeReviewerIndex]
                if (activeReviewer) {
                  void requestReviewer(activeReviewer)
                  return
                }
                void handleRequestReview()
                return
              }
              if (event.key === 'Escape') {
                event.preventDefault()
                handleReviewerPickerOpenChange(false)
              }
            }}
          />
        </div>
        <div className="max-h-[300px] overflow-y-auto scrollbar-sleek">
          {reviewerMetadata.loading ? (
            <div className="px-3 py-2 text-[13px] text-muted-foreground">Loading…</div>
          ) : filteredReviewerCandidates.length > 0 ? (
            <>
              {suggestedReviewerRows.length > 0 ? (
                <>
                  <div className="border-b border-border/70 bg-muted/50 px-3 py-1.5 text-[12px] font-semibold text-foreground">
                    Suggestions
                  </div>
                  {suggestedReviewerRows.map((reviewer, index) =>
                    renderReviewerPickerRow(reviewer, { suggested: true, activeIndex: index })
                  )}
                </>
              ) : null}
              <div className="border-b border-border/70 bg-muted/50 px-3 py-1.5 text-[12px] font-semibold text-foreground">
                Everyone else
              </div>
              {everyoneElseReviewerRows.length > 0 ? (
                everyoneElseReviewerRows.map((reviewer, index) =>
                  renderReviewerPickerRow(reviewer, {
                    suggested: false,
                    activeIndex: suggestedReviewerRows.length + index
                  })
                )
              ) : (
                <div className="px-3 py-2 text-[13px] text-muted-foreground">
                  No matching reviewers.
                </div>
              )}
            </>
          ) : (
            <div className="px-3 py-2 text-[13px] text-muted-foreground">
              {reviewerMetadata.error ??
                (hasReviewerMetadata
                  ? 'No matching reviewers.'
                  : 'Open the PR details to view current reviewers.')}
            </div>
          )}
        </div>
      </PopoverContent>
    </Popover>
  )
}

export function PRChecksCell({
  item,
  onOpen,
  onLoadChecks
}: {
  item: GitHubWorkItem
  onOpen: () => void
  onLoadChecks: () => void
}): React.JSX.Element {
  const triggerRef = useRef<HTMLButtonElement | null>(null)

  useEffect(() => {
    if (item.type !== 'pr' || item.checksSummary) {
      return
    }
    const node = triggerRef.current
    if (!node || typeof IntersectionObserver === 'undefined') {
      return
    }
    let requested = false
    const observer = new IntersectionObserver(
      (entries) => {
        if (requested || !entries.some((entry) => entry.isIntersecting)) {
          return
        }
        requested = true
        onLoadChecks()
        observer.disconnect()
      },
      { rootMargin: '160px 0px' }
    )
    observer.observe(node)
    return () => observer.disconnect()
  }, [item.checksSummary, item.type, onLoadChecks])

  if (item.type !== 'pr') {
    return <span className="text-[11px] text-muted-foreground">Issue</span>
  }
  const summary = item.checksSummary
  const Icon =
    summary?.state === 'success'
      ? CheckCircle2
      : summary?.state === 'failure'
        ? AlertCircle
        : summary?.state === 'pending'
          ? Clock3
          : Minus
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          ref={triggerRef}
          type="button"
          onFocus={onLoadChecks}
          onMouseEnter={onLoadChecks}
          onClick={(event) => {
            event.stopPropagation()
            onLoadChecks()
            onOpen()
          }}
          className={cn(
            'inline-flex max-w-full items-center gap-1 rounded-full border px-2 py-0.5 text-[10px] font-medium transition hover:brightness-110',
            getChecksTone(item)
          )}
        >
          <Icon className="size-3" />
          <span className="truncate">{getChecksLabel(item)}</span>
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={6}>
        Open PR checks
      </TooltipContent>
    </Tooltip>
  )
}

export function PRMergeCell({
  item,
  repo,
  onRefresh
}: {
  item: GitHubWorkItem
  repo: Repo | null
  onRefresh: () => void
}): React.JSX.Element {
  const [merging, setMerging] = useState(false)
  const confirm = useConfirmationDialog()
  if (item.type !== 'pr') {
    return <span className="text-[11px] text-muted-foreground">Issue</span>
  }
  const mergePresentation = presentGitHubPRMergeState(item)
  const mergeMethods = resolveGitHubPRMergeMethods(item.mergeMethodSettings)
  const mergeDisabled = !repo || merging || !mergePresentation.directMergeAvailable

  const handleMerge = async (method: GitHubPRMergeMethod): Promise<void> => {
    if (!repo || mergeDisabled) {
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
    setMerging(true)
    try {
      const result = await api.gh.mergePR({
        repoPath: repo.path,
        repoId: repo.id,
        prNumber: item.number,
        method,
        prRepo: item.prRepo ?? null
      })
      if (result.ok) {
        toast.success('Pull request merged')
        onRefresh()
      } else {
        toast.error(result.error)
      }
    } catch {
      toast.error('Failed to merge pull request')
    } finally {
      setMerging(false)
    }
  }

  const handleAutoMerge = async (): Promise<void> => {
    if (!repo || !mergePresentation.autoMergeAction) {
      return
    }
    const enabled = mergePresentation.autoMergeAction.kind === 'enable'
    setMerging(true)
    try {
      const result = await api.gh.setPRAutoMerge({
        repoPath: repo.path,
        repoId: repo.id,
        prNumber: item.number,
        enabled,
        prRepo: item.prRepo ?? null
      })
      if (result.ok) {
        toast.success(enabled ? 'Auto-merge enabled' : 'Auto-merge disabled')
        onRefresh()
      } else {
        toast.error(result.error)
      }
    } catch {
      toast.error(enabled ? 'Failed to enable auto-merge' : 'Failed to disable auto-merge')
    } finally {
      setMerging(false)
    }
  }

  return (
    <DropdownMenu modal={false}>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              onClick={(event) => event.stopPropagation()}
              className={cn(
                'inline-flex max-w-full items-center gap-1 rounded-full border px-2 py-0.5 text-[10px] font-medium transition hover:brightness-110',
                mergePresentation.tone
              )}
            >
              {merging ? (
                <LoaderCircle className="size-3 animate-spin" />
              ) : (
                <GitMerge className="size-3" />
              )}
              <span className="truncate">{mergePresentation.label}</span>
              <ChevronDown className="size-2.5 opacity-60" />
            </button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={6}>
          {mergePresentation.tooltip}
        </TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="start" onClick={(event) => event.stopPropagation()}>
        {mergePresentation.autoMergeAction && (
          <DropdownMenuItem disabled={!repo || merging} onSelect={() => void handleAutoMerge()}>
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
  )
}

// Why: builds the page number array with ellipsis gaps, matching GitHub's
// pagination pattern: always show first page, last page, and a window of
// pages around the current page with "..." gaps between distant ranges.
function getPageNumbers(current: number, total: number): (number | 'ellipsis')[] {
  if (total <= 9) {
    return Array.from({ length: total }, (_, i) => i)
  }
  const pages = new Set<number>()
  pages.add(0)
  pages.add(total - 1)
  for (let i = Math.max(0, current - 2); i <= Math.min(total - 1, current + 2); i++) {
    pages.add(i)
  }
  const sorted = [...pages].sort((a, b) => a - b)
  const result: (number | 'ellipsis')[] = []
  for (let i = 0; i < sorted.length; i++) {
    if (i > 0 && sorted[i] - sorted[i - 1] > 1) {
      result.push('ellipsis')
    }
    result.push(sorted[i])
  }
  return result
}

export function PaginationBar({
  currentPage,
  totalPages,
  loadingTarget,
  onPageChange
}: {
  currentPage: number
  totalPages: number
  loadingTarget: number | null
  onPageChange: (page: number) => void
}): React.JSX.Element {
  const pageNumbers = getPageNumbers(currentPage, totalPages)
  const btnClass =
    'inline-flex w-24 items-center justify-center gap-0.5 rounded-md px-2 py-1 text-sm text-muted-foreground transition hover:bg-muted/60 hover:text-foreground disabled:pointer-events-none disabled:opacity-40'
  const numClass = (page: number): string =>
    cn(
      'inline-flex size-8 items-center justify-center rounded-md text-sm transition',
      page === currentPage
        ? 'bg-primary text-primary-foreground font-medium'
        : 'text-muted-foreground hover:bg-muted/60 hover:text-foreground'
    )

  return (
    <nav
      aria-label="Pagination"
      className="flex items-center justify-center gap-1 border-t border-border/50 px-4 py-3"
    >
      <button
        type="button"
        disabled={currentPage === 0 || loadingTarget !== null}
        onClick={() => onPageChange(currentPage - 1)}
        aria-label="Previous page"
        className={btnClass}
      >
        <ChevronLeft className="size-4" />
        Previous
      </button>

      {pageNumbers.map((entry, idx) =>
        entry === 'ellipsis' ? (
          <span
            key={`ellipsis-${idx}`}
            aria-hidden
            className="inline-flex size-8 items-center justify-center text-sm text-muted-foreground"
          >
            &hellip;
          </span>
        ) : (
          <button
            key={entry}
            type="button"
            disabled={loadingTarget !== null && loadingTarget !== entry}
            onClick={() => onPageChange(entry)}
            aria-label={`Page ${entry + 1}`}
            aria-current={entry === currentPage ? 'page' : undefined}
            className={numClass(entry)}
          >
            {loadingTarget === entry ? (
              <LoaderCircle className="size-3.5 animate-spin" />
            ) : (
              entry + 1
            )}
          </button>
        )
      )}

      <button
        type="button"
        disabled={currentPage >= totalPages - 1 || loadingTarget !== null}
        onClick={() => onPageChange(currentPage + 1)}
        aria-label="Next page"
        className={btnClass}
      >
        Next
        <ChevronRight className="size-4" />
      </button>
    </nav>
  )
}
