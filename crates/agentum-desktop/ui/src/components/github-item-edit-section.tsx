import { api } from '@/tauri'
import type { GitHubItemDialogProjectOrigin } from './github-item-types'
import { getStateLabel, getStateTone } from '@/lib/github-work-item-state'
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ArrowRight, ChevronDown, CircleDashed, CircleDot, ExternalLink, FolderKanban, LoaderCircle, Pencil, Plus, Settings } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { ButtonGroup } from '@/components/ui/button-group'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/store'
import { useIssueProjectStatus } from '@/components/sidebar/IssueProjectStatusChip'
import { useRepoLabels, useRepoAssignees, useImmediateMutation } from '@/hooks/useIssueMetadata'
import { useRepoLabelsBySlug, useRepoAssigneesBySlug } from '@/hooks/useGitHubSlugMetadata'
import type { GitHubWorkItem } from '../../../shared/types'

// Why: the GH item dialog can be opened from any work-item list surface and
// doesn't have the full owner/repo context the list's cache entry carries.
// Parsing the canonical `https://github.com/{owner}/{repo}/...` URL is the
// simplest reliable source — the URL is already present on every work item
// and survives the main-process → IPC boundary. Non-GitHub hosts return null,
// which matches the indicator's suppression rule.
function getGitHubRepositoryLabelsUrl(itemUrl: string): string | null {
  try {
    const parsed = new URL(itemUrl)
    if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') {
      return null
    }
    const segments = parsed.pathname.split('/').filter(Boolean)
    if (segments.length < 2) {
      return null
    }
    // Why: label management is repository-scoped; preserving the origin keeps
    // GitHub Enterprise URLs working while navigating away from the issue path.
    parsed.pathname = `/${segments[0]}/${segments[1]}/labels`
    parsed.search = ''
    parsed.hash = ''
    return parsed.toString()
  } catch {
    return null
  }
}

function GitHubLabelsSettingsLink({
  url,
  separated,
  onOpen
}: {
  url: string | null
  separated?: boolean
  onOpen?: () => void
}): React.JSX.Element | null {
  if (!url) {
    return null
  }

  return (
    <div className={cn(separated && 'mt-1 border-t border-border/60 pt-1')}>
      <button
        type="button"
        onClick={() => {
          onOpen?.()
          void api.shell.openUrl(url)
        }}
        className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-[12px] text-muted-foreground hover:bg-accent hover:text-accent-foreground"
      >
        <Settings className="size-3.5 shrink-0" />
        <span className="min-w-0 flex-1 text-left">Edit labels on GitHub</span>
        <ExternalLink className="size-3 shrink-0 opacity-70" />
      </button>
    </div>
  )
}

export function GHEditSection({
  item,
  repoPath,
  repoId,
  projectOrigin,
  localState,
  localLabels,
  onStateChange,
  onLabelsChange,
  onMutated,
  assignees,
  onUse,
  onOpenOrUse,
  attachedWorkspaceLabel,
  layout = 'horizontal'
}: {
  item: GitHubWorkItem
  repoPath: string | null
  repoId: string | null
  projectOrigin: GitHubItemDialogProjectOrigin | undefined
  localState: GitHubWorkItem['state']
  localLabels: string[]
  onStateChange: (state: GitHubWorkItem['state']) => void
  onLabelsChange: (labels: string[]) => void
  /** Why: called after a successful issue mutation so the parent dialog can
   *  invalidate its work-item-details cache entry. Without this, reopening the
   *  drawer in the FRESH_MS window would paint pre-mutation data. */
  onMutated: () => void
  assignees: string[]
  onUse: (item: GitHubWorkItem) => void
  onOpenOrUse?: (item: GitHubWorkItem) => void
  attachedWorkspaceLabel?: string | null
  /** `'horizontal'` is the legacy strip rendered above the conversation; the
   *  `'sidebar'` layout matches the GitHub issue page's right rail with each
   *  metadata row stacked under a section heading. */
  layout?: 'horizontal' | 'sidebar'
}): React.JSX.Element | null {
  const [labelPopoverOpen, setLabelPopoverOpen] = useState(false)
  const [assigneePopoverOpen, setAssigneePopoverOpen] = useState(false)
  const [localAssignees, setLocalAssignees] = useState<string[]>(assignees)
  // #379: the tracker's REAL status — the Projects v2 board column — shown in
  // the sidebar's Status section beside open/closed. The hook live-refreshes
  // on tracker.phase_changed bus events, so engine/MCP transitions appear
  // without reopening. Null (unbound / off-project / error) renders nothing.
  const projectColumn = useIssueProjectStatus({
    open: layout === 'sidebar' && item.type === 'issue',
    issueUrl: item.url,
    workdir: repoPath ?? undefined,
    repoId: repoId ?? undefined
  })
  const editedAssigneesItemKeyRef = useRef<string | null>(null)
  const assigneesItemKey = `${item.repoId}\0${item.id}`
  const patchWorkItem = useAppStore((s) => s.patchWorkItem)
  const patchProjectRowContent = useAppStore((s) => s.patchProjectRowContent)
  const { isPending, run } = useImmediateMutation()
  // Why: when the dialog opens from a Project view, mutations route through
  // *BySlug IPCs and we must keep `projectViewCache` in sync alongside
  // `workItemsCache` — `patchWorkItem` only walks the latter, so without this
  // helper the Project table would render stale data until manual refresh.
  // See docs/design/github-project-view-tasks.md §Dialog editing from Project rows.
  const patchProjectRowIfNeeded = useCallback(
    (patch: Parameters<typeof patchProjectRowContent>[2]) => {
      if (!projectOrigin) {
        return
      }
      patchProjectRowContent(projectOrigin.cacheKey, projectOrigin.projectItemId, patch)
    },
    [projectOrigin, patchProjectRowContent]
  )

  // Why: when projectOrigin is set we MUST read labels/assignees from the
  // row's repo, not from the workspace path — otherwise the popovers list
  // values from a different repo than the writes target.
  const slugOwner = projectOrigin?.owner ?? null
  const slugRepo = projectOrigin?.repo ?? null
  const repoLabelsByPath = useRepoLabels(
    projectOrigin ? null : repoPath,
    projectOrigin ? null : repoId
  )
  const repoLabelsBySlug = useRepoLabelsBySlug(slugOwner, slugRepo)
  const repoLabels = projectOrigin ? repoLabelsBySlug : repoLabelsByPath
  const repositoryLabelsUrl = useMemo(() => getGitHubRepositoryLabelsUrl(item.url), [item.url])
  const repoAssigneesByPath = useRepoAssignees(
    projectOrigin ? null : repoPath,
    projectOrigin ? null : repoId
  )
  const repoAssigneesBySlug = useRepoAssigneesBySlug(slugOwner, slugRepo, assignees)
  const repoAssignees = projectOrigin ? repoAssigneesBySlug : repoAssigneesByPath
  const hasAttachedWorkspace =
    attachedWorkspaceLabel !== null && attachedWorkspaceLabel !== undefined
  const handleOpenOrUseWorkspace = useCallback((): void => {
    if (onOpenOrUse) {
      onOpenOrUse(item)
      return
    }
    onUse(item)
  }, [item, onOpenOrUse, onUse])

  // Why: sync local assignees when item changes or when the detail fetch
  // resolves with real data — but skip if the user already made an
  // optimistic edit so we don't clobber in-flight changes.
  useEffect(() => {
    if (editedAssigneesItemKeyRef.current === assigneesItemKey) {
      return
    }
    setLocalAssignees(assignees)
  }, [assigneesItemKey, assignees])

  const handleStateChange = useCallback(
    (newState: 'open' | 'closed') => {
      if (newState === localState) {
        return
      }
      const prevState = localState
      run('state', {
        mutate: () =>
          runIssueUpdate({
            repoId: item.repoId,
            repoPath,
            projectOrigin,
            number: item.number,
            updates: { state: newState }
          }),
        onOptimistic: () => {
          onStateChange(newState)
          patchWorkItem(item.id, { state: newState }, item.repoId)
          patchProjectRowIfNeeded({ state: newState })
        },
        onRevert: () => {
          onStateChange(prevState)
          patchWorkItem(item.id, { state: prevState }, item.repoId)
          patchProjectRowIfNeeded({ state: prevState })
        },
        onSuccess: () => {
          patchWorkItem(item.id, { state: newState }, item.repoId)
          patchProjectRowIfNeeded({ state: newState })
          onMutated()
        },
        onError: (err) => toast.error(err)
      })
    },
    [
      item.id,
      item.number,
      item.repoId,
      localState,
      repoPath,
      projectOrigin,
      patchWorkItem,
      patchProjectRowIfNeeded,
      run,
      onStateChange,
      onMutated
    ]
  )

  const handleLabelToggle = useCallback(
    (label: string) => {
      const isAdding = !localLabels.includes(label)
      const prevLabels = localLabels
      const newLabels = isAdding ? [...prevLabels, label] : prevLabels.filter((l) => l !== label)

      if (isAdding) {
        run('labels', {
          mutate: () =>
            runIssueUpdate({
              repoId: item.repoId,
              repoPath,
              projectOrigin,
              number: item.number,
              updates: { addLabels: [label] }
            }),
          onOptimistic: () => {
            onLabelsChange(newLabels)
            patchWorkItem(item.id, { labels: newLabels }, item.repoId)
            patchProjectRowIfNeeded({ labels: newLabels })
          },
          onSuccess: () => {
            onMutated()
          },
          onRevert: () => {
            onLabelsChange(prevLabels)
            patchWorkItem(item.id, { labels: prevLabels }, item.repoId)
            patchProjectRowIfNeeded({ labels: prevLabels })
          },
          onError: (err) => toast.error(err)
        })
      } else {
        run('labels', {
          mutate: () =>
            runIssueUpdate({
              repoId: item.repoId,
              repoPath,
              projectOrigin,
              number: item.number,
              updates: { removeLabels: [label] }
            }),
          onOptimistic: () => {
            onLabelsChange(newLabels)
            patchWorkItem(item.id, { labels: newLabels }, item.repoId)
            patchProjectRowIfNeeded({ labels: newLabels })
          },
          onRevert: () => {
            onLabelsChange(prevLabels)
            patchWorkItem(item.id, { labels: prevLabels }, item.repoId)
            patchProjectRowIfNeeded({ labels: prevLabels })
          },
          onSuccess: () => {
            onMutated()
          },
          onError: (err) => toast.error(err)
        })
      }
    },
    [
      item.id,
      item.number,
      item.repoId,
      localLabels,
      repoPath,
      projectOrigin,
      patchWorkItem,
      patchProjectRowIfNeeded,
      run,
      onLabelsChange,
      onMutated
    ]
  )

  const handleAssigneeToggle = useCallback(
    (login: string) => {
      const isAssigned = localAssignees.includes(login)
      const prevAssignees = localAssignees
      const newAssignees = isAssigned
        ? prevAssignees.filter((l) => l !== login)
        : [...prevAssignees, login]

      // Why: the optimistic guard is scoped to this repo item so switching
      // items does not suppress the next item's assignee sync.
      editedAssigneesItemKeyRef.current = assigneesItemKey
      if (isAssigned) {
        run('assignees', {
          mutate: () =>
            runIssueUpdate({
              repoId: item.repoId,
              repoPath,
              projectOrigin,
              number: item.number,
              updates: { removeAssignees: [login] }
            }),
          onOptimistic: () => {
            setLocalAssignees(newAssignees)
            patchProjectRowIfNeeded({ assignees: newAssignees })
          },
          onRevert: () => {
            setLocalAssignees(prevAssignees)
            patchProjectRowIfNeeded({ assignees: prevAssignees })
          },
          onSuccess: () => {
            onMutated()
          },
          onError: (err) => toast.error(err)
        })
      } else {
        run('assignees', {
          mutate: () =>
            runIssueUpdate({
              repoId: item.repoId,
              repoPath,
              projectOrigin,
              number: item.number,
              updates: { addAssignees: [login] }
            }),
          onOptimistic: () => {
            setLocalAssignees(newAssignees)
            patchProjectRowIfNeeded({ assignees: newAssignees })
          },
          onSuccess: () => {
            onMutated()
          },
          onRevert: () => {
            setLocalAssignees(prevAssignees)
            patchProjectRowIfNeeded({ assignees: prevAssignees })
          },
          onError: (err) => toast.error(err)
        })
      }
    },
    [
      item.number,
      item.repoId,
      assigneesItemKey,
      repoPath,
      projectOrigin,
      localAssignees,
      patchProjectRowIfNeeded,
      run,
      onMutated
    ]
  )

  if (item.type === 'pr') {
    return null
  }

  const checkIcon = (
    <svg className="size-2.5" viewBox="0 0 12 12" fill="none">
      <path
        d="M2 6l3 3 5-5"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  )

  if (layout === 'sidebar') {
    return (
      <aside className="flex flex-col gap-5 text-[13px]">
        {/* State */}
        <section>
          <div className="mb-2 text-[11px] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
            Status
          </div>
          <Popover>
            <PopoverTrigger asChild>
              <button
                type="button"
                className={cn(
                  'inline-flex w-full items-center justify-between gap-2 rounded-md border px-2.5 py-1.5 text-[12px] font-medium transition hover:brightness-125 hover:ring-1 hover:ring-white/10',
                  getStateTone({ ...item, state: localState })
                )}
              >
                <span className="inline-flex items-center gap-1.5">
                  {localState === 'closed' ? (
                    <CircleDashed className="size-3.5" />
                  ) : (
                    <CircleDot className="size-3.5" />
                  )}
                  {getStateLabel({ ...item, state: localState })}
                </span>
                <ChevronDown className="size-3 opacity-60" />
              </button>
            </PopoverTrigger>
            <PopoverContent className="w-44 p-1" align="start">
              <button
                type="button"
                onClick={() => handleStateChange('open')}
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
                onClick={() => handleStateChange('closed')}
                className={cn(
                  'flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-[12px] hover:bg-accent',
                  localState === 'closed' && 'bg-accent/50'
                )}
              >
                <CircleDashed className="size-3 text-rose-500" />
                Closed
              </button>
            </PopoverContent>
          </Popover>
          {projectColumn ? (
            <div className="mt-2 flex items-center gap-1.5 rounded-md border border-indigo-500/25 bg-indigo-500/5 px-2.5 py-1.5 text-[12px]">
              <FolderKanban className="size-3.5 text-indigo-500" />
              <span className="text-muted-foreground">Board</span>
              <span className="ml-auto font-medium text-indigo-600 dark:text-indigo-300">
                {projectColumn}
              </span>
            </div>
          ) : null}
        </section>

        {/* Assignees */}
        <section>
          <div className="mb-2 flex items-center justify-between text-[11px] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
            <span>Assignees</span>
            <Popover open={assigneePopoverOpen} onOpenChange={setAssigneePopoverOpen}>
              <PopoverTrigger asChild>
                <button
                  type="button"
                  disabled={isPending('assignees') || repoAssignees.loading}
                  aria-label="Edit assignees"
                  className="rounded p-0.5 text-muted-foreground transition hover:bg-accent hover:text-foreground disabled:opacity-50"
                >
                  {isPending('assignees') ? (
                    <LoaderCircle className="size-3 animate-spin" />
                  ) : (
                    <Pencil className="size-3" />
                  )}
                </button>
              </PopoverTrigger>
              <PopoverContent
                className="popover-scroll-content scrollbar-sleek w-60 p-1"
                align="end"
              >
                {repoAssignees.error ? (
                  <div className="px-2 py-3 text-center text-[12px] text-destructive">
                    {repoAssignees.error}
                  </div>
                ) : (
                  <div>
                    {repoAssignees.data.map((user) => (
                      <button
                        key={user.login}
                        type="button"
                        onClick={() => handleAssigneeToggle(user.login)}
                        className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-[12px] hover:bg-accent"
                      >
                        <span
                          className={cn(
                            'flex size-3.5 items-center justify-center rounded-sm border',
                            localAssignees.includes(user.login)
                              ? 'border-primary bg-primary text-primary-foreground'
                              : 'border-input'
                          )}
                        >
                          {localAssignees.includes(user.login) && checkIcon}
                        </span>
                        <span className="min-w-0 flex-1 text-left">
                          <span className="block truncate">{user.login}</span>
                          {user.name && (
                            <span className="block truncate text-[11px] text-muted-foreground">
                              {user.name}
                            </span>
                          )}
                        </span>
                      </button>
                    ))}
                  </div>
                )}
              </PopoverContent>
            </Popover>
          </div>
          {localAssignees.length === 0 ? (
            <div className="text-[12px] text-muted-foreground">No one assigned</div>
          ) : (
            <ul className="flex flex-col gap-1.5">
              {localAssignees.map((login) => {
                const user = repoAssignees.data.find((u) => u.login === login)
                return (
                  <li key={login} className="flex min-w-0 items-center gap-2">
                    {user?.avatarUrl ? (
                      <img
                        src={user.avatarUrl}
                        alt=""
                        className="size-5 shrink-0 rounded-full border border-border/40 object-cover"
                      />
                    ) : (
                      <div className="size-5 shrink-0 rounded-full bg-muted" />
                    )}
                    <span className="min-w-0 truncate text-[12px] font-medium text-foreground">
                      {login}
                    </span>
                  </li>
                )
              })}
            </ul>
          )}
        </section>

        {/* Labels */}
        <section>
          <div className="mb-2 flex items-center justify-between text-[11px] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
            <span>Labels</span>
            <Popover open={labelPopoverOpen} onOpenChange={setLabelPopoverOpen}>
              <PopoverTrigger asChild>
                <button
                  type="button"
                  disabled={isPending('labels') || repoLabels.loading}
                  aria-label="Edit labels"
                  className="rounded p-0.5 text-muted-foreground transition hover:bg-accent hover:text-foreground disabled:opacity-50"
                >
                  {isPending('labels') ? (
                    <LoaderCircle className="size-3 animate-spin" />
                  ) : (
                    <Pencil className="size-3" />
                  )}
                </button>
              </PopoverTrigger>
              <PopoverContent
                className="popover-scroll-content scrollbar-sleek w-60 p-1"
                align="end"
              >
                {repoLabels.error ? (
                  <div className="px-2 py-3 text-center text-[12px] text-destructive">
                    {repoLabels.error}
                  </div>
                ) : null}
                {!repoLabels.error ? (
                  <div>
                    {repoLabels.data.map((label) => (
                      <button
                        key={label}
                        type="button"
                        onClick={() => handleLabelToggle(label)}
                        className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-[12px] hover:bg-accent"
                      >
                        <span
                          className={cn(
                            'flex size-3.5 items-center justify-center rounded-sm border',
                            localLabels.includes(label)
                              ? 'border-primary bg-primary text-primary-foreground'
                              : 'border-input'
                          )}
                        >
                          {localLabels.includes(label) && checkIcon}
                        </span>
                        {label}
                      </button>
                    ))}
                  </div>
                ) : null}
                <GitHubLabelsSettingsLink
                  url={repositoryLabelsUrl}
                  separated={!repoLabels.error && repoLabels.data.length > 0}
                  onOpen={() => setLabelPopoverOpen(false)}
                />
              </PopoverContent>
            </Popover>
          </div>
          {localLabels.length === 0 ? (
            <div className="text-[12px] text-muted-foreground">None yet</div>
          ) : (
            <div className="flex flex-wrap gap-1.5">
              {localLabels.map((name) => (
                <span
                  key={name}
                  className="inline-flex items-center rounded-full border border-border/50 bg-muted/40 px-2 py-0.5 text-[11px] font-medium text-foreground"
                >
                  {name}
                </span>
              ))}
            </div>
          )}
        </section>

        <section>
          <div className="mb-2 text-[11px] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
            Workspace
          </div>
          {attachedWorkspaceLabel ? (
            <div className="mb-2 flex min-w-0 items-center gap-1.5 text-[12px] text-muted-foreground">
              <FolderKanban className="size-3.5 shrink-0" />
              <span className="truncate">{attachedWorkspaceLabel}</span>
            </div>
          ) : null}
          {hasAttachedWorkspace ? (
            <DropdownMenu modal={false}>
              <ButtonGroup className="w-full">
                <Button
                  type="button"
                  size="sm"
                  onClick={handleOpenOrUseWorkspace}
                  className="flex-1 gap-1.5"
                  aria-label="Open workspace attached to issue"
                >
                  Open workspace
                  <ArrowRight className="size-3.5" />
                </Button>
                <DropdownMenuTrigger asChild>
                  <Button type="button" size="icon-sm" aria-label="More issue workspace actions">
                    <ChevronDown className="size-3.5" />
                  </Button>
                </DropdownMenuTrigger>
              </ButtonGroup>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onSelect={() => onUse(item)}>
                  <Plus className="size-4" />
                  Start new workspace
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          ) : (
            <Button
              type="button"
              size="sm"
              onClick={() => onUse(item)}
              className="w-full gap-1.5"
              aria-label="Start workspace from issue"
            >
              Start workspace from issue
              <ArrowRight className="size-3.5" />
            </Button>
          )}
        </section>
      </aside>
    )
  }

  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border/60 px-4 py-2.5">
      {/* State */}
      <Popover>
        <PopoverTrigger asChild>
          <button
            type="button"
            className={cn(
              'group/status inline-flex items-center gap-0.5 rounded-full border px-2 py-0.5 text-[11px] font-medium transition hover:brightness-125 hover:ring-1 hover:ring-white/10',
              getStateTone({ ...item, state: localState })
            )}
          >
            {getStateLabel({ ...item, state: localState })}
            <ChevronDown className="size-2.5 opacity-50" />
          </button>
        </PopoverTrigger>
        <PopoverContent className="w-36 p-1" align="start">
          <button
            type="button"
            onClick={() => handleStateChange('open')}
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
            onClick={() => handleStateChange('closed')}
            className={cn(
              'flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-[12px] hover:bg-accent',
              localState === 'closed' && 'bg-accent/50'
            )}
          >
            <CircleDashed className="size-3 text-rose-500" />
            Closed
          </button>
        </PopoverContent>
      </Popover>

      {/* Labels */}
      <Popover open={labelPopoverOpen} onOpenChange={setLabelPopoverOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            disabled={isPending('labels') || repoLabels.loading}
            className="group/labels inline-flex items-center gap-1 rounded-full border border-border/30 bg-muted/20 px-2 py-0.5 text-[11px] transition hover:brightness-125 hover:ring-1 hover:ring-white/10 disabled:opacity-50"
          >
            {localLabels.length === 0 ? (
              <span className="text-muted-foreground">+ Label</span>
            ) : (
              localLabels.map((name) => (
                <span key={name} className="text-[10px] text-muted-foreground">
                  {name}
                </span>
              ))
            )}
            {isPending('labels') ? (
              <LoaderCircle className="size-3 animate-spin text-muted-foreground" />
            ) : (
              <ChevronDown className="size-2.5 opacity-50" />
            )}
          </button>
        </PopoverTrigger>
        <PopoverContent className="popover-scroll-content scrollbar-sleek w-52 p-1" align="start">
          {repoLabels.error ? (
            <div className="px-2 py-3 text-center text-[12px] text-destructive">
              {repoLabels.error}
            </div>
          ) : null}
          {!repoLabels.error ? (
            <div>
              {repoLabels.data.map((label) => (
                <button
                  key={label}
                  type="button"
                  onClick={() => handleLabelToggle(label)}
                  className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-[12px] hover:bg-accent"
                >
                  <span
                    className={cn(
                      'flex size-3.5 items-center justify-center rounded-sm border',
                      localLabels.includes(label)
                        ? 'border-primary bg-primary text-primary-foreground'
                        : 'border-input'
                    )}
                  >
                    {localLabels.includes(label) && checkIcon}
                  </span>
                  {label}
                </button>
              ))}
            </div>
          ) : null}
          <GitHubLabelsSettingsLink
            url={repositoryLabelsUrl}
            separated={!repoLabels.error && repoLabels.data.length > 0}
            onOpen={() => setLabelPopoverOpen(false)}
          />
        </PopoverContent>
      </Popover>

      {/* Assignees */}
      <Popover open={assigneePopoverOpen} onOpenChange={setAssigneePopoverOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            disabled={isPending('assignees') || repoAssignees.loading}
            className="group/assignees inline-flex items-center gap-1 rounded-full border border-border/30 bg-muted/20 px-2 py-0.5 text-[11px] transition hover:brightness-125 hover:ring-1 hover:ring-white/10 disabled:opacity-50"
          >
            {localAssignees.length === 0 ? (
              <span className="text-muted-foreground">+ Assignee</span>
            ) : (
              localAssignees.map((login) => (
                <span key={login} className="text-[10px] text-muted-foreground">
                  {login}
                </span>
              ))
            )}
            {isPending('assignees') ? (
              <LoaderCircle className="size-3 animate-spin text-muted-foreground" />
            ) : (
              <ChevronDown className="size-2.5 opacity-50" />
            )}
          </button>
        </PopoverTrigger>
        <PopoverContent className="popover-scroll-content scrollbar-sleek w-52 p-1" align="start">
          {repoAssignees.error ? (
            <div className="px-2 py-3 text-center text-[12px] text-destructive">
              {repoAssignees.error}
            </div>
          ) : (
            <div>
              {repoAssignees.data.map((user) => (
                <button
                  key={user.login}
                  type="button"
                  onClick={() => handleAssigneeToggle(user.login)}
                  className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-[12px] hover:bg-accent"
                >
                  <span
                    className={cn(
                      'flex size-3.5 items-center justify-center rounded-sm border',
                      localAssignees.includes(user.login)
                        ? 'border-primary bg-primary text-primary-foreground'
                        : 'border-input'
                    )}
                  >
                    {localAssignees.includes(user.login) && checkIcon}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate">{user.login}</span>
                    {user.name && (
                      <span className="block truncate text-[11px] text-muted-foreground">
                        {user.name}
                      </span>
                    )}
                  </span>
                </button>
              ))}
            </div>
          )}
        </PopoverContent>
      </Popover>

      <div className="ml-auto flex min-w-0 items-center gap-2">
        {attachedWorkspaceLabel ? (
          <span className="inline-flex min-w-0 items-center gap-1 text-[11px] text-muted-foreground">
            <FolderKanban className="size-3 shrink-0" />
            <span className="truncate">{attachedWorkspaceLabel}</span>
          </span>
        ) : null}
        {hasAttachedWorkspace ? (
          <DropdownMenu modal={false}>
            <ButtonGroup>
              <Button
                type="button"
                size="sm"
                onClick={handleOpenOrUseWorkspace}
                className="gap-2"
                aria-label="Open workspace attached to issue"
              >
                Open workspace
                <ArrowRight className="size-4" />
              </Button>
              <DropdownMenuTrigger asChild>
                <Button type="button" size="icon-sm" aria-label="More issue workspace actions">
                  <ChevronDown className="size-3.5" />
                </Button>
              </DropdownMenuTrigger>
            </ButtonGroup>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => onUse(item)}>
                <Plus className="size-4" />
                Start new workspace
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        ) : (
          <Button
            type="button"
            size="sm"
            onClick={() => onUse(item)}
            className="gap-2"
            aria-label="Start workspace from issue"
          >
            Start workspace from issue
            <ArrowRight className="size-4" />
          </Button>
        )}
      </div>
    </div>
  )
}
