import { api } from '@/tauri'
import { parseOwnerRepoFromItemUrl } from '@/lib/github-item-url'
import { buildRequestedReviewUsers, mergeReviewerSuggestions } from '@/lib/github-reviewers'
import { ReviewerAvatar } from './github-item-display'
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ArrowDown, ArrowUp, Check, LoaderCircle, Users, X } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/store'
import { callRuntimeRpc, getActiveRuntimeTarget } from '@/runtime/runtime-rpc-client'
import { useRepoAssignees } from '@/hooks/useIssueMetadata'
import { useRepoAssigneesBySlug } from '@/hooks/useGitHubSlugMetadata'
import { getGitHubPRReviewerRows, normalizeGitHubReviewerLogins } from '@/components/github-pr-reviewer-display'
import type { GitHubWorkItem, GitHubAssignableUser } from '../../../shared/types'

export function PRReviewersPanel({
  item,
  loading,
  repoPath,
  onReviewersRequested
}: {
  item: GitHubWorkItem
  loading: boolean
  repoPath: string | null
  onReviewersRequested: (reviewRequests: GitHubAssignableUser[]) => void
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [reviewerInput, setReviewerInput] = useState('')
  const [reviewerPickerSide, setReviewerPickerSide] = useState<'top' | 'bottom'>('bottom')
  const [reviewerPickerMaxHeight, setReviewerPickerMaxHeight] = useState<number | null>(null)
  const [activeReviewerCursor, setActiveReviewerCursor] = useState({ resetKey: '', index: 0 })
  const [submitting, setSubmitting] = useState(false)
  const [localReviewRequests, setLocalReviewRequests] = useState<GitHubAssignableUser[]>(
    () => item.reviewRequests ?? []
  )
  const [reviewRequestsSource, setReviewRequestsSource] = useState(() => ({
    itemId: item.id,
    repoId: item.repoId,
    reviewRequests: item.reviewRequests
  }))
  const patchWorkItem = useAppStore((s) => s.patchWorkItem)
  const settings = useAppStore((s) => s.settings)
  const reviewerInputRef = useRef<HTMLInputElement | null>(null)
  const reviewerInputFocusFrameRef = useRef<number | null>(null)
  const reviewerPanelMountedRef = useRef(true)

  const cancelReviewerInputFocusFrame = useCallback((): void => {
    if (reviewerInputFocusFrameRef.current !== null) {
      cancelAnimationFrame(reviewerInputFocusFrameRef.current)
      reviewerInputFocusFrameRef.current = null
    }
  }, [])

  const scheduleReviewerInputFocus = useCallback((): void => {
    if (!reviewerPanelMountedRef.current) {
      return
    }
    cancelReviewerInputFocusFrame()
    reviewerInputFocusFrameRef.current = requestAnimationFrame(() => {
      reviewerInputFocusFrameRef.current = null
      reviewerInputRef.current?.focus()
    })
  }, [cancelReviewerInputFocusFrame])

  useEffect(() => {
    reviewerPanelMountedRef.current = true
    return () => {
      reviewerPanelMountedRef.current = false
      cancelReviewerInputFocusFrame()
    }
  }, [cancelReviewerInputFocusFrame])

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

  const reviewSlug = useMemo(() => parseOwnerRepoFromItemUrl(item.url), [item.url])
  const reviewerMetadataBySlug = useRepoAssigneesBySlug(
    open && reviewSlug ? reviewSlug.owner : null,
    open && reviewSlug ? reviewSlug.repo : null,
    reviewerSeedUsers.map((user) => user.login),
    settings
  )
  const reviewerMetadataByPath = useRepoAssignees(
    open && !reviewSlug ? repoPath : null,
    open && !reviewSlug ? item.repoId : null
  )
  const reviewerMetadata = reviewSlug ? reviewerMetadataBySlug : reviewerMetadataByPath
  const displayItem = { ...item, reviewRequests: localReviewRequests }
  const reviewers = getGitHubPRReviewerRows(displayItem)
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

  const hasReviewerMetadata =
    item.reviewDecision !== undefined ||
    localReviewRequests.length > 0 ||
    item.reviewRequests !== undefined ||
    item.latestReviews !== undefined
  const canRequestReview = !!repoPath || getActiveRuntimeTarget(settings).kind === 'environment'

  const measureReviewerPickerPlacement = useCallback(() => {
    const rect = reviewerInputRef.current?.getBoundingClientRect()
    if (!rect) {
      setReviewerPickerSide('bottom')
      setReviewerPickerMaxHeight(null)
      return
    }

    const gap = 8
    const minUsefulHeight = 180
    const availableBelow = window.innerHeight - rect.bottom - gap
    const availableAbove = rect.top - gap
    const nextSide =
      availableBelow < minUsefulHeight && availableAbove > availableBelow ? 'top' : 'bottom'
    const available = nextSide === 'top' ? availableAbove : availableBelow

    setReviewerPickerSide(nextSide)
    setReviewerPickerMaxHeight(Math.max(120, Math.min(330, available)))
  }, [])

  const handleRequestReview = async (requestedLogins?: string[]): Promise<void> => {
    if (submitting) {
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
    const target = getActiveRuntimeTarget(settings)
    if (target.kind !== 'environment' && !repoPath) {
      toast.error('No repo context available for this pull request.')
      return
    }
    setSubmitting(true)
    try {
      const result =
        target.kind === 'environment'
          ? await callRuntimeRpc<{ ok: boolean; error?: string }>(
              target,
              'github.requestPRReviewers',
              { repo: item.repoId, prNumber: item.number, reviewers: logins },
              { timeoutMs: 30_000 }
            )
          : await api.gh.requestPRReviewers({
              repoPath: repoPath ?? '',
              repoId: item.repoId,
              prNumber: item.number,
              reviewers: logins
            })
      if (!reviewerPanelMountedRef.current) {
        return
      }
      if (!result.ok) {
        toast.error(result.error ?? 'Failed to request reviewer')
        return
      }
      const nextReviewRequests = buildRequestedReviewUsers(
        logins,
        reviewerCandidates,
        localReviewRequests
      )
      setLocalReviewRequests(nextReviewRequests)
      patchWorkItem(item.id, { reviewRequests: nextReviewRequests }, item.repoId)
      onReviewersRequested(nextReviewRequests)
      setReviewerInput('')
      toast.success(logins.length === 1 ? 'Reviewer requested' : 'Reviewers requested')
    } catch {
      if (reviewerPanelMountedRef.current) {
        toast.error('Failed to request reviewer')
      }
    } finally {
      if (reviewerPanelMountedRef.current) {
        setSubmitting(false)
      }
    }
  }

  const handleRemoveReviewers = async (reviewersToRemove: string[]): Promise<void> => {
    if (submitting) {
      return
    }
    const selected = new Set(localReviewRequests.map((reviewer) => reviewer.login.toLowerCase()))
    const logins = reviewersToRemove
      .map((reviewer) => reviewer.trim().replace(/^@/, ''))
      .filter((reviewer) => reviewer.length > 0 && selected.has(reviewer.toLowerCase()))
    if (logins.length === 0) {
      return
    }
    const target = getActiveRuntimeTarget(settings)
    if (target.kind !== 'environment' && !repoPath) {
      toast.error('No repo context available for this pull request.')
      return
    }
    setSubmitting(true)
    try {
      const result =
        target.kind === 'environment'
          ? await callRuntimeRpc<{ ok: boolean; error?: string }>(
              target,
              'github.removePRReviewers',
              { repo: item.repoId, prNumber: item.number, reviewers: logins },
              { timeoutMs: 30_000 }
            )
          : await api.gh.removePRReviewers({
              repoPath: repoPath ?? '',
              repoId: item.repoId,
              prNumber: item.number,
              reviewers: logins
            })
      if (!reviewerPanelMountedRef.current) {
        return
      }
      if (!result.ok) {
        toast.error(result.error ?? 'Failed to remove reviewer')
        return
      }
      const removed = new Set(logins.map((login) => login.toLowerCase()))
      const nextReviewRequests = localReviewRequests.filter(
        (reviewer) => !removed.has(reviewer.login.toLowerCase())
      )
      setLocalReviewRequests(nextReviewRequests)
      patchWorkItem(item.id, { reviewRequests: nextReviewRequests }, item.repoId)
      onReviewersRequested(nextReviewRequests)
      setReviewerInput('')
      toast.success(logins.length === 1 ? 'Reviewer removed' : 'Reviewers removed')
    } catch {
      if (reviewerPanelMountedRef.current) {
        toast.error('Failed to remove reviewer')
      }
    } finally {
      if (reviewerPanelMountedRef.current) {
        setSubmitting(false)
      }
    }
  }

  const requestReviewer = async (reviewer: GitHubAssignableUser): Promise<void> => {
    await (selectedReviewerLogins.has(reviewer.login.toLowerCase())
      ? handleRemoveReviewers([reviewer.login])
      : handleRequestReview([reviewer.login]))
    scheduleReviewerInputFocus()
  }

  const handleReviewerPickerOpenChange = (nextOpen: boolean): void => {
    if (nextOpen) {
      measureReviewerPickerPlacement()
    }
    setOpen(nextOpen)
    if (nextOpen) {
      scheduleReviewerInputFocus()
      return
    }
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
        aria-label={
          selected ? `Unrequest reviewer ${reviewer.login}` : `Request reviewer ${reviewer.login}`
        }
        aria-pressed={selected}
        className={cn(
          'flex min-h-10 w-full items-center gap-2 border-b border-border/70 px-3 py-2 text-left text-[13px] outline-none last:border-b-0 hover:bg-accent/70 focus-visible:bg-accent focus-visible:text-accent-foreground',
          active && 'bg-accent text-accent-foreground',
          selected && 'font-medium'
        )}
        onMouseEnter={() => setActiveReviewerIndex(options.activeIndex)}
        onMouseDown={(event) => {
          event.preventDefault()
        }}
        onFocus={() => setActiveReviewerIndex(options.activeIndex)}
        onClick={() => {
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
              Recently edited these files
            </span>
          ) : null}
        </span>
      </button>
    )
  }

  return (
    <aside className="rounded-lg border border-border/50 bg-card/50 shadow-xs">
      <div className="flex h-10 items-center gap-2 border-b border-border/50 px-3">
        <Users className="size-3.5 text-muted-foreground" />
        <span className="text-[13px] font-medium text-foreground">Reviewers</span>
        {reviewers.length > 0 ? (
          <span className="ml-auto rounded-full border border-border/50 bg-muted/30 px-1.5 py-0.5 text-[11px] tabular-nums text-muted-foreground">
            {reviewers.length}
          </span>
        ) : null}
      </div>
      <div className="px-3 py-2.5">
        {loading && !hasReviewerMetadata ? (
          <div className="flex items-center gap-2 py-1 text-[12px] text-muted-foreground">
            <LoaderCircle className="size-3.5 animate-spin" />
            Loading reviewers
          </div>
        ) : reviewers.length > 0 ? (
          <div className="flex flex-col gap-2">
            {reviewers.map((reviewer) => {
              const canRemoveReviewer = selectedReviewerLogins.has(reviewer.login.toLowerCase())
              return (
                <div key={reviewer.login} className="flex min-w-0 items-center gap-2">
                  <ReviewerAvatar login={reviewer.login} avatarUrl={reviewer.avatarUrl} />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[13px] font-medium text-foreground">
                      {reviewer.login}
                    </div>
                    {reviewer.name ? (
                      <div className="truncate text-[11px] text-muted-foreground">
                        {reviewer.name}
                      </div>
                    ) : null}
                  </div>
                  <span className="shrink-0 text-[11px] text-muted-foreground">
                    {reviewer.stateLabel}
                  </span>
                  {canRemoveReviewer ? (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          className="size-6 shrink-0 text-muted-foreground hover:text-foreground"
                          disabled={submitting || !canRequestReview}
                          aria-label={`Remove reviewer ${reviewer.login}`}
                          onClick={() => {
                            void handleRemoveReviewers([reviewer.login])
                          }}
                        >
                          <X className="size-3.5" />
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent>Remove reviewer</TooltipContent>
                    </Tooltip>
                  ) : null}
                </div>
              )
            })}
          </div>
        ) : (
          <div className="py-1 text-[12px] text-muted-foreground">No reviewers requested.</div>
        )}
        <Popover open={open} onOpenChange={handleReviewerPickerOpenChange}>
          <PopoverAnchor asChild>
            <Input
              ref={reviewerInputRef}
              value={reviewerInput}
              onChange={(event) => {
                setReviewerInput(event.target.value)
                if (!open) {
                  handleReviewerPickerOpenChange(true)
                }
              }}
              disabled={submitting || !canRequestReview}
              placeholder="Type or choose a user"
              aria-label="Reviewer"
              aria-expanded={open}
              aria-haspopup="listbox"
              className="mt-3 h-8 min-w-0 cursor-text rounded-md border-border/50 bg-background text-xs"
              onFocus={() => {
                if (canRequestReview) {
                  handleReviewerPickerOpenChange(true)
                }
              }}
              onClick={() => {
                if (canRequestReview) {
                  handleReviewerPickerOpenChange(true)
                }
              }}
              onKeyDown={(event) => {
                if (event.key === 'ArrowDown' && actionableReviewerRows.length > 0) {
                  event.preventDefault()
                  setOpen(true)
                  setActiveReviewerIndex((current) => (current + 1) % actionableReviewerRows.length)
                  return
                }
                if (event.key === 'ArrowUp' && actionableReviewerRows.length > 0) {
                  event.preventDefault()
                  setOpen(true)
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
          </PopoverAnchor>
          <PopoverContent
            className="flex w-[330px] flex-col overflow-hidden rounded-md border-border/70 p-0"
            align="start"
            side={reviewerPickerSide}
            sideOffset={6}
            avoidCollisions={false}
            style={{
              maxHeight: reviewerPickerMaxHeight ? `${reviewerPickerMaxHeight}px` : undefined
            }}
            onOpenAutoFocus={(event) => {
              event.preventDefault()
            }}
          >
            <div className="border-b border-border/70 px-3 py-2">
              <div className="text-[13px] font-semibold text-foreground">
                Request up to 15 reviewers
              </div>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto scrollbar-sleek">
              {reviewerMetadata.loading ? (
                <div className="px-3 py-2 text-[13px] text-muted-foreground">Loading...</div>
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
      </div>
    </aside>
  )
}
