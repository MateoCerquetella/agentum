import { api } from '@/tauri'
import { ConversationTab, PRFilesCombinedDiffViewer } from './github-item-conversation'
import { GHEditSection } from './github-item-edit-section'
import { ChecksTab } from './github-item-checks-tab'
import type { GitHubItemDialogProjectOrigin } from './github-item-types'
import { WorkItemIssueSourceIndicator } from './github-item-issue-source-indicator'
import { parseOwnerRepoFromItemUrl } from '@/lib/github-item-url'
import { getWorkItemDetailsCacheKey } from '@/lib/github-pr-detail-helpers'
import { normalizeItemDialogTab } from '@/lib/github-work-item-state'
import type { ItemDialogTab } from '@/shared/types'
export type { ItemDialogTab }
import { getWorkItemDetailsForRepo, setPRFileViewedForRepo } from '@/lib/github-repo-operations'
import { WorkItemStateBadge } from './github-item-display'
import { formatRelativeTime } from '@/lib/relative-time'
/* eslint-disable max-lines -- Why: the GH item dialog keeps its header, conversation, files, and checks tabs co-located so the read-only PR/Issue surface stays in one place while this view evolves. */
import React, { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { ArrowRight, Check, ChevronDown, ChevronLeft, CircleDashed, CircleDot, Copy, ExternalLink, FileText, FolderKanban, GitPullRequest, ListChecks, LoaderCircle, MessageSquare, Plus, X } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { ButtonGroup } from '@/components/ui/button-group'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { VisuallyHidden } from 'radix-ui'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import CommentMarkdown from '@/components/sidebar/CommentMarkdown'
import { cn } from '@/lib/utils'
import {
  clearGitHubLinkCopied,
  createGitHubLinkCopyState,
  markGitHubLinkCopied,
  resolveGitHubLinkCopyState
} from '@/components/github-link-copy-state'
import { useAppStore } from '@/store'
import { useAllWorktrees } from '@/store/selectors'
import { callRuntimeRpc, getActiveRuntimeTarget } from '@/runtime/runtime-rpc-client'
import { findGithubIssueWorkspaceAttachment, getGithubWorkItemWorkspaceAttachmentLabel } from '@/lib/github-work-item-workspace-attachment'
import { activateAndRevealWorktree } from '@/lib/worktree-activation'
import type { GitHubPRFileViewedState, GitHubWorkItem, GitHubWorkItemDetails, GitHubAssignableUser, PRCheckDetail, PRComment } from '../../../shared/types'

const IS_MAC = navigator.userAgent.includes('Mac')

type GitHubItemDialogProps = {
  workItem: GitHubWorkItem | null
  repoPath: string | null
  repoId?: string | null
  initialTab?: ItemDialogTab
  variant?: 'sheet' | 'page'
  backLabel?: string
  /** Called when the user clicks the primary CTA to start work from this item. */
  onUse: (item: GitHubWorkItem) => void
  onReviewRequestsChange?: (
    itemKey: { id: string; repoId: string },
    reviewRequests: GitHubAssignableUser[]
  ) => void
  onClose: () => void
  /** Optional Project-origin context. When set, edits in the dialog are
   *  routed via slug-addressed mutation IPCs against the row's actual repo
   *  instead of the active workspace's `repoPath`. Both can be set
   *  simultaneously (Project mode where the row also lives in the active
   *  workspace) — slug routing wins for writes. */
  projectOrigin?: GitHubItemDialogProjectOrigin
}

// Why: SWR cache for the work-item details fetch. Reopening the same drawer
// pays full IPC + `gh` process startup latency without this; with it, cached
// data paints immediately while a background refetch keeps the view honest.
// Cache is keyed by repoPath + issueSourcePreference + type + number so
// upstream/origin source toggles and issue#N vs pr#N never collide. Bounded
// to ~50 entries to cap memory; entries older than FRESH_MS trigger a
// background refetch on open. See docs/gh-work-item-drawer-cache.md.
const WORK_ITEM_DETAILS_CACHE_MAX = 50
const WORK_ITEM_DETAILS_FRESH_MS = 30_000
type WorkItemDetailsCacheEntry = {
  details: GitHubWorkItemDetails | null
  fetchedAt: number
  pending?: Promise<GitHubWorkItemDetails | null>
  error?: string
}
const workItemDetailsCache = new Map<string, WorkItemDetailsCacheEntry>()

// Why: drawers subscribe via useSyncExternalStore so reopening a cached item
// paints synchronously on first render. Stability of the snapshot relies on
// every cache write replacing the entry object identity (delete+set), which
// touchWorkItemDetailsCache already does.
const workItemDetailsCacheListeners = new Set<() => void>()
function subscribeWorkItemDetailsCache(listener: () => void): () => void {
  workItemDetailsCacheListeners.add(listener)
  return () => {
    workItemDetailsCacheListeners.delete(listener)
  }
}
function notifyWorkItemDetailsCache(): void {
  for (const listener of workItemDetailsCacheListeners) {
    listener()
  }
}

function touchWorkItemDetailsCache(key: string, entry: WorkItemDetailsCacheEntry): void {
  // Why: re-insert to move to MRU position; Map preserves insertion order so
  // the oldest key is always first when evicting.
  workItemDetailsCache.delete(key)
  workItemDetailsCache.set(key, entry)
  while (workItemDetailsCache.size > WORK_ITEM_DETAILS_CACHE_MAX) {
    const oldest = workItemDetailsCache.keys().next().value
    if (oldest === undefined) {
      break
    }
    workItemDetailsCache.delete(oldest)
  }
  notifyWorkItemDetailsCache()
}

// Why: exposed so mutation handlers (in this file and elsewhere) can drop a
// stale entry after a successful local mutation. Cross-window invalidation
// arrives via the `gh:workItemMutated` event listener installed below.
export function invalidateWorkItemDetailsCacheForKey(key: string): void {
  // Why: bump generation so an in-flight fetch launched before this exact-key
  // invalidation will not write its stale result back into the cache.
  workItemDetailsCacheGeneration += 1
  const existed = workItemDetailsCache.delete(key)
  if (existed) {
    notifyWorkItemDetailsCache()
  }
}

// Why: monotonically increases on every invalidation so an in-flight refetch
// that started before a mutation can detect that its result is stale and
// must not be written back. Without this, a mutation that lands while a
// refetch is in flight would have its invalidation silently undone when the
// stale promise resolves and re-populates the entry.
let workItemDetailsCacheGeneration = 0

// Why: when we don't have the exact cache key (e.g. an event from another
// window only carries repoPath + number + type), drop every entry that
// matches the (repoPath, type, number) tuple regardless of source preference.
function invalidateWorkItemDetailsCacheByMatch(args: {
  repoPath: string
  repoId?: string
  type: 'issue' | 'pr'
  number: number
}): void {
  workItemDetailsCacheGeneration += 1
  const suffix = `\0${args.type}\0${args.number}`
  const prefix = `${args.repoId ?? args.repoPath}\0`
  let removed = false
  for (const key of Array.from(workItemDetailsCache.keys())) {
    if (key.startsWith(prefix) && key.endsWith(suffix)) {
      workItemDetailsCache.delete(key)
      removed = true
    }
  }
  if (removed) {
    notifyWorkItemDetailsCache()
  }
}

function patchCachedPRFileViewedState(
  cacheKey: string,
  path: string,
  viewerViewedState: GitHubPRFileViewedState
): GitHubPRFileViewedState | undefined {
  const prev = workItemDetailsCache.get(cacheKey)
  const files = prev?.details?.files
  if (!prev?.details || !files) {
    return undefined
  }
  let previousState: GitHubPRFileViewedState | undefined
  const nextFiles = files.map((file) => {
    if (file.path !== path) {
      return file
    }
    previousState = file.viewerViewedState ?? 'UNVIEWED'
    return { ...file, viewerViewedState }
  })
  if (previousState === undefined || previousState === viewerViewedState) {
    return previousState
  }
  touchWorkItemDetailsCache(cacheKey, {
    ...prev,
    details: { ...prev.details, files: nextFiles },
    error: undefined
  })
  return previousState
}

function patchCachedPRChecks(cacheKey: string, checks: PRCheckDetail[]): void {
  const prev = workItemDetailsCache.get(cacheKey)
  if (!prev?.details) {
    return
  }
  touchWorkItemDetailsCache(cacheKey, {
    ...prev,
    details: { ...prev.details, checks },
    fetchedAt: Date.now(),
    error: undefined
  })
}

function patchCachedPRReviewRequests(
  cacheKey: string,
  reviewRequests: GitHubAssignableUser[]
): void {
  const prev = workItemDetailsCache.get(cacheKey)
  if (!prev?.details) {
    return
  }
  touchWorkItemDetailsCache(cacheKey, {
    ...prev,
    details: {
      ...prev.details,
      item: { ...prev.details.item, reviewRequests }
    },
    fetchedAt: Date.now(),
    error: undefined
  })
}

function patchCachedWorkItemBody(cacheKey: string, body: string): void {
  const prev = workItemDetailsCache.get(cacheKey)
  if (!prev?.details) {
    return
  }
  touchWorkItemDetailsCache(cacheKey, {
    ...prev,
    details: { ...prev.details, body },
    fetchedAt: Date.now(),
    error: undefined
  })
}

// Why: install once at module load — every dialog instance shares the cache,
// so a single subscription is enough. The preload bridge re-emits the
// main-process broadcast for every window, so each renderer invalidates its
// own cache when any window's mutation lands. We track the unsubscribe so
// Vite HMR doesn't accumulate listeners across module reloads in dev.
let workItemMutatedUnsub: (() => void) | undefined
if (typeof window !== 'undefined' && api?.gh?.onWorkItemMutated) {
  workItemMutatedUnsub = api.gh.onWorkItemMutated((payload) => {
    invalidateWorkItemDetailsCacheByMatch({
      repoPath: payload.repoPath,
      repoId: payload.repoId,
      type: payload.type,
      number: payload.number
    })
  })
}
if (typeof import.meta !== 'undefined' && import.meta.hot) {
  import.meta.hot.dispose(() => {
    workItemMutatedUnsub?.()
  })
}

async function runPullRequestStateUpdate(args: {
  repoPath: string | null
  repoId?: string | null
  projectOrigin: GitHubItemDialogProjectOrigin | undefined
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
    const res =
      target.kind === 'environment'
        ? await callRuntimeRpc<Awaited<ReturnType<typeof api.gh.updatePullRequestBySlug>>>(
            target,
            'github.project.updatePullRequestBySlug',
            updateArgs,
            { timeoutMs: 30_000 }
          )
        : await api.gh.updatePullRequestBySlug(updateArgs)
    if (!res.ok) {
      throw new Error(res.error.message)
    }
    return
  }
  if (!args.repoPath) {
    throw new Error('No repo context available for this pull request.')
  }
  const res = await api.gh.updatePRState({
    repoPath: args.repoPath,
    repoId: args.repoId ?? undefined,
    prNumber: args.number,
    updates: args.updates
  })
  if (!res.ok) {
    throw new Error(res.error)
  }
}

export default function GitHubItemDialog({
  workItem,
  repoPath,
  repoId,
  initialTab,
  variant = 'sheet',
  backLabel = 'Back',
  projectOrigin,
  onUse,
  onReviewRequestsChange,
  onClose
}: GitHubItemDialogProps): React.JSX.Element {
  const workItemId = workItem?.id
  const [tab, setTab] = useState<ItemDialogTab>(() => normalizeItemDialogTab(workItem, initialTab))
  const [localState, setLocalState] = useState<GitHubWorkItem['state']>(workItem?.state ?? 'open')
  const [localLabels, setLocalLabels] = useState<string[]>(workItem?.labels ?? [])
  const [linkCopyState, setLinkCopyState] = useState(() => createGitHubLinkCopyState(workItemId))
  const resolvedLinkCopyState = resolveGitHubLinkCopyState(linkCopyState, workItemId)
  if (resolvedLinkCopyState !== linkCopyState) {
    // Why: switching GitHub items should not paint a stale copied indicator
    // from the previous item while waiting for a passive Effect pass.
    setLinkCopyState(resolvedLinkCopyState)
  }
  const linkCopied = resolvedLinkCopyState.copied
  const workItemState = workItem?.state
  const workItemLabels = workItem?.labels
  const effectiveRepoId = repoId ?? workItem?.repoId ?? null
  const allWorktrees = useAllWorktrees()
  const issueAttachedWorkspace = useMemo(
    () =>
      workItem?.type === 'issue'
        ? findGithubIssueWorkspaceAttachment(allWorktrees, effectiveRepoId, workItem.number)
        : null,
    [allWorktrees, effectiveRepoId, workItem]
  )
  const issueAttachedWorkspaceLabel = issueAttachedWorkspace
    ? getGithubWorkItemWorkspaceAttachmentLabel(issueAttachedWorkspace)
    : null

  const handleOpenOrUseIssueWorkspace = useCallback(
    (item: GitHubWorkItem): void => {
      const currentAttached = findGithubIssueWorkspaceAttachment(
        useAppStore.getState().allWorktrees(),
        effectiveRepoId,
        item.number
      )
      if (!currentAttached) {
        onUse(item)
        return
      }

      const result = activateAndRevealWorktree(currentAttached.id)
      if (result === false) {
        toast.error('Unable to open the workspace attached to this issue.')
      }
    },
    [effectiveRepoId, onUse]
  )

  // Why: the cache key has to include the issue source preference so a user
  // toggling between origin/upstream for the same issue number doesn't read
  // back the wrong repo's details. We pull it from the repos slice rather
  // than threading it as a prop because every existing call site already has
  // the repo registered in the store.
  const issueSourcePreference = useAppStore((s) => {
    if (!repoPath && !effectiveRepoId) {
      return undefined
    }
    return s.repos.find((r) => (effectiveRepoId ? r.id === effectiveRepoId : r.path === repoPath))
      ?.issueSourcePreference
  })
  const detailsCacheKey = useMemo(() => {
    if (!workItem || !repoPath || !effectiveRepoId) {
      return null
    }
    return getWorkItemDetailsCacheKey({
      repoPath,
      repoId: effectiveRepoId,
      issueSourcePreference,
      type: workItem.type,
      number: workItem.number
    })
  }, [repoPath, effectiveRepoId, workItem, issueSourcePreference])

  // Why: reset lifted edit state when the dialog switches items or when the
  // same item receives an optimistic cache patch from the surrounding table.
  useEffect(() => {
    if (workItemState && workItemLabels) {
      setLocalState(workItemState)
      setLocalLabels(workItemLabels)
    }
  }, [workItemId, workItemState, workItemLabels])

  // Why: track comments added optimistically before the detail fetch resolves
  // so they can be merged into the fetch result instead of being overwritten.
  const optimisticCommentsRef = useRef<PRComment[]>([])
  // Why: track the last item we fetched so we can distinguish "reopen same
  // item" from "switch to a different item". Reopening the same item must
  // preserve optimistic comments because gh's 60s response cache will return
  // stale data that doesn't include the just-posted comment.
  const prevItemIdRef = useRef<string | null>(null)

  // Why: when this dialog opens immediately after another Radix overlay
  // (e.g. the New Issue dialog) closed, Radix may leave `pointer-events: none`
  // on <body>. That silently kills clicks on the header's Close/open-in-GitHub
  // buttons. Poll a few frames to clear it whenever Radix re-applies it during
  // its own mount sequence.
  useEffect(() => {
    if (!workItem) {
      return
    }
    let cancelled = false
    let count = 0
    let frameId: number | null = null
    const tick = (): void => {
      frameId = null
      if (cancelled) {
        return
      }
      if (document.body.style.pointerEvents === 'none') {
        document.body.style.pointerEvents = ''
      }
      if (count++ < 5) {
        frameId = requestAnimationFrame(tick)
      }
    }
    tick()
    return () => {
      cancelled = true
      if (frameId !== null) {
        cancelAnimationFrame(frameId)
      }
    }
  }, [workItem])

  // Why: subscribe to the module-level cache so reopening a cached item
  // paints synchronously on first render. getSnapshot returns the entry
  // object directly — touchWorkItemDetailsCache writes always replace entry
  // identity (delete+set), so Map.get is referentially stable between writes.
  const cachedEntry = useSyncExternalStore(
    subscribeWorkItemDetailsCache,
    useCallback(
      () => (detailsCacheKey ? workItemDetailsCache.get(detailsCacheKey) : undefined),
      [detailsCacheKey]
    )
  )

  // Why: bumped by appendOptimisticComment on cold open (no cached details
  // yet) so the details memo re-runs and surfaces the optimistic comment via
  // the loading-shell fallback. Without this, the comment would sit in the
  // ref alone and not render until the in-flight fetch lands. The cache
  // notify path handles the warm case.
  const [optimisticTick, setOptimisticTick] = useState(0)

  // Why: merge optimistic comments into the cached details. Keyed off
  // cachedEntry identity (stable) rather than the optimistic ref array (a
  // fresh array each render) to avoid unnecessary recomputation. Cache
  // notifications after optimistic writes will re-render this anyway.
  const details = useMemo<GitHubWorkItemDetails | null>(() => {
    const cachedDetails = cachedEntry?.details ?? null
    const opt = optimisticCommentsRef.current
    if (!cachedDetails) {
      // Why: details may still be loading on a cold open — surface optimistic
      // comments via a minimal shell so a comment posted before the fetch
      // resolves isn't held invisibly in ref-land.
      if (opt.length > 0 && workItem) {
        return { item: workItem, body: '', comments: [...opt] }
      }
      return null
    }
    if (opt.length === 0) {
      return cachedDetails
    }
    const ids = new Set(cachedDetails.comments.map((c) => c.id))
    const missing = opt.filter((c) => !ids.has(c.id))
    if (missing.length === 0) {
      return cachedDetails
    }
    return { ...cachedDetails, comments: [...cachedDetails.comments, ...missing] }
    // Why: optimisticTick is the rerender signal for cold-open writes — the
    // memo reads optimisticCommentsRef.current (a ref, no subscription), so
    // bumping the tick is what forces this memo to re-run. The lint flags it
    // as "unnecessary" because it's not referenced in the body, but removing
    // it would silently break the cold-open optimistic-shell path.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cachedEntry, workItem, optimisticTick])

  const loading = !!cachedEntry?.pending && !cachedEntry?.details
  const error = cachedEntry?.error && !cachedEntry?.details ? cachedEntry.error : null
  const detailsLoaded =
    Boolean(cachedEntry?.details) ||
    Boolean(cachedEntry && !cachedEntry.pending && !cachedEntry.error && cachedEntry.fetchedAt > 0)

  // Why: if a cross-window mutation invalidates the open drawer's entry
  // (cachedEntry becomes undefined while workItem is still set), the main
  // fetch effect won't re-run because its deps haven't changed. Bump a local
  // tick so the fetch effect fires a refetch in that case.
  const [refetchTick, setRefetchTick] = useState(0)
  useEffect(() => {
    if (workItem && detailsCacheKey && !cachedEntry) {
      setRefetchTick((n) => n + 1)
    }
  }, [workItem, detailsCacheKey, cachedEntry])

  useEffect(() => {
    if (!workItem || !repoPath || !detailsCacheKey) {
      return
    }
    // Why: only clear optimistic comments when switching to a genuinely
    // different item. When reopening the same item (close → reopen), the
    // gh API's 60s response cache will return stale data that omits the
    // just-posted comment — preserving the optimistic ref lets the merge
    // logic above re-attach it to the stale response.
    if (workItem.id !== prevItemIdRef.current) {
      optimisticCommentsRef.current = []
    }
    prevItemIdRef.current = workItem.id
    setTab(normalizeItemDialogTab(workItem, initialTab))

    const cached = workItemDetailsCache.get(detailsCacheKey)
    const now = Date.now()
    const hasFreshData = cached?.details && now - cached.fetchedAt <= WORK_ITEM_DETAILS_FRESH_MS

    if (hasFreshData) {
      return
    }

    // Why: dedupe concurrent opens for the same key — concurrent dialogs or
    // a rapid close→reopen must share one in-flight promise instead of
    // racing two `gh` subprocesses against each other.
    const inflight: Promise<GitHubWorkItemDetails | null> =
      cached?.pending ??
      getWorkItemDetailsForRepo({
        repoPath,
        repoId: effectiveRepoId ?? undefined,
        number: workItem.number,
        type: workItem.type
      })

    // Why: snapshot the invalidation generation at fetch start; if the
    // generation advances before we resolve, a mutation invalidated the
    // entry mid-flight and we must not write a stale result back.
    const launchedAtGeneration = workItemDetailsCacheGeneration

    if (!cached?.pending) {
      touchWorkItemDetailsCache(detailsCacheKey, {
        details: cached?.details ?? null,
        fetchedAt: cached?.fetchedAt ?? 0,
        pending: inflight,
        error: cached?.error
      })
    }

    inflight
      .then((result) => {
        const invalidatedMidFlight = workItemDetailsCacheGeneration !== launchedAtGeneration
        const prev = workItemDetailsCache.get(detailsCacheKey)
        if (invalidatedMidFlight) {
          // Why: entry was deliberately dropped; do not recreate it. If the
          // entry still exists (later open repopulated it) leave it alone too.
          return
        }
        // Why: 404/unauthorized must not overwrite valid cached data. When the
        // IPC resolves to null and we already have cached details, keep the
        // stale data — only blank entries get the null payload.
        if (result === null && prev?.details) {
          touchWorkItemDetailsCache(detailsCacheKey, {
            details: prev.details,
            fetchedAt: prev.fetchedAt,
            error: undefined
          })
        } else {
          touchWorkItemDetailsCache(detailsCacheKey, {
            details: result,
            fetchedAt: Date.now(),
            error: undefined
          })
        }
      })
      .catch((err) => {
        const message = err instanceof Error ? err.message : 'Failed to load details'
        const invalidatedMidFlight = workItemDetailsCacheGeneration !== launchedAtGeneration
        if (invalidatedMidFlight) {
          return
        }
        const prev = workItemDetailsCache.get(detailsCacheKey)
        // Why: stale-on-error — keep cached data if we have it, drop the
        // pending promise so the next open can retry. Only surface the
        // blocking error when nothing is cached.
        touchWorkItemDetailsCache(detailsCacheKey, {
          details: prev?.details ?? null,
          fetchedAt: prev?.fetchedAt ?? 0,
          error: message
        })
      })
  }, [repoPath, effectiveRepoId, workItem, detailsCacheKey, initialTab, refetchTick])

  const Icon = workItem?.type === 'pr' ? GitPullRequest : CircleDot
  const displayWorkItem = useMemo<GitHubWorkItem | null>(() => {
    if (!workItem) {
      return null
    }
    if (!details?.item) {
      return workItem
    }
    return { ...workItem, ...details.item, repoId: workItem.repoId }
  }, [details?.item, workItem])

  useEffect(() => {
    if (!workItem || details?.item.reviewRequests === undefined) {
      return
    }
    // Why: PR details can carry fresher reviewer metadata than the list row;
    // push it back so the Tasks review chip doesn't keep a stale snapshot.
    onReviewRequestsChange?.(
      { id: workItem.id, repoId: workItem.repoId },
      details.item.reviewRequests
    )
  }, [details?.item.reviewRequests, onReviewRequestsChange, workItem])

  const body = details?.body ?? ''
  const comments = details?.comments ?? []
  const files = details?.files ?? []
  const checks = details?.checks ?? []
  const [pendingViewedPaths, setPendingViewedPaths] = useState<Set<string>>(() => new Set())
  // Why: clipboard IPC can resolve after the dialog unmounts; skip copied-state
  // feedback instead of starting its reset timer on a stale surface.
  const linkCopyMountedRef = useRef(false)
  const linkCopiedResetTimerRef = useRef<number | null>(null)
  const clearLinkCopiedResetTimer = useCallback((): void => {
    if (linkCopiedResetTimerRef.current === null) {
      return
    }
    window.clearTimeout(linkCopiedResetTimerRef.current)
    linkCopiedResetTimerRef.current = null
  }, [])
  const setLinkCopyButtonRef = useCallback(
    (node: HTMLButtonElement | null) => {
      linkCopyMountedRef.current = node !== null
      if (node === null) {
        // Why: the copied-state timer belongs to the copy control surface;
        // clear it when that surface detaches without a passive cleanup Effect.
        clearLinkCopiedResetTimer()
      }
    },
    [clearLinkCopiedResetTimer]
  )

  const handleCopyWorkItemLink = useCallback(async (): Promise<void> => {
    if (!workItem) {
      return
    }
    try {
      // Why: Electron's clipboard IPC is reliable even when browser clipboard
      // APIs lose focus/activation inside nested overlay surfaces.
      await api.ui.writeClipboardText(workItem.url)
      if (!linkCopyMountedRef.current) {
        return
      }
      clearLinkCopiedResetTimer()
      const copiedWorkItemId = workItem.id
      setLinkCopyState(markGitHubLinkCopied(copiedWorkItemId))
      linkCopiedResetTimerRef.current = window.setTimeout(() => {
        linkCopiedResetTimerRef.current = null
        setLinkCopyState((current) => clearGitHubLinkCopied(current, copiedWorkItemId))
      }, 1500)
      toast.success('GitHub link copied')
    } catch {
      toast.error('Failed to copy GitHub link')
    }
  }, [clearLinkCopiedResetTimer, workItem])

  const appendOptimisticComment = useCallback(
    (comment: PRComment) => {
      // Why: skip refreshDetails() — gh api --cache 60s returns stale data
      // that overwrites the optimistic comment. The next dialog open (after
      // cache expiry) will pick up the server-confirmed version.
      optimisticCommentsRef.current.push(comment)
      // Why: write through the module-level cache so subscribers (this
      // drawer plus any concurrent ones on the same item) re-render with the
      // optimistic comment. Mark fetchedAt as stale (0) so the next open
      // still triggers a background refresh to pick up server-side fields
      // like reaction groups or thread bindings.
      if (detailsCacheKey) {
        const prev = workItemDetailsCache.get(detailsCacheKey)
        if (prev?.details) {
          const ids = new Set(prev.details.comments.map((c) => c.id))
          if (!ids.has(comment.id)) {
            touchWorkItemDetailsCache(detailsCacheKey, {
              details: { ...prev.details, comments: [...prev.details.comments, comment] },
              fetchedAt: 0,
              error: undefined
            })
            return
          }
        }
      }
      // Why: when the cache has no details yet (still loading), no cache
      // write/notify fires above. Bump local state so the details memo
      // re-runs and surfaces the optimistic comment via the loading-shell
      // fallback instead of holding it invisibly in the ref.
      setOptimisticTick((n) => n + 1)
    },
    [detailsCacheKey]
  )

  const handlePRFileViewedChange = useCallback(
    async (path: string, viewed: boolean): Promise<boolean> => {
      if (!repoPath || !details?.pullRequestId || !workItem || workItem.type !== 'pr') {
        toast.error('Unable to sync viewed state for this pull request.')
        return false
      }
      setPendingViewedPaths((prev) => new Set(prev).add(path))
      const nextState: GitHubPRFileViewedState = viewed ? 'VIEWED' : 'UNVIEWED'
      const previousState = detailsCacheKey
        ? patchCachedPRFileViewedState(detailsCacheKey, path, nextState)
        : undefined
      try {
        const ok = await setPRFileViewedForRepo({
          repoId: workItem.repoId,
          repoPath,
          prNumber: workItem.number,
          pullRequestId: details.pullRequestId,
          path,
          viewed
        })
        if (!ok) {
          if (detailsCacheKey && previousState) {
            patchCachedPRFileViewedState(detailsCacheKey, path, previousState)
          }
          toast.error('Failed to sync viewed state with GitHub.')
          return false
        }
        return true
      } finally {
        setPendingViewedPaths((prev) => {
          const next = new Set(prev)
          next.delete(path)
          return next
        })
      }
    },
    [details?.pullRequestId, detailsCacheKey, repoPath, workItem]
  )

  const isIssuePage = variant === 'page' && workItem?.type === 'issue'
  const ownerRepo = workItem ? parseOwnerRepoFromItemUrl(workItem.url) : null
  const issueStateBadgeTone =
    localState === 'closed' ? 'bg-rose-600 text-white' : 'bg-emerald-600 text-white'

  const content = workItem ? (
    <div className="flex h-full min-h-0 flex-col">
      {isIssuePage ? (
        <>
          {/* Row 1: breadcrumb-style strip mirroring GitHub's canvas-subtle header */}
          <div className="flex-none border-b border-border/60 bg-muted/30 px-6 py-2.5">
            <div className="flex items-center gap-2 text-[13px] text-muted-foreground">
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={onClose}
                className="-ml-2 h-7 gap-1 px-2 text-muted-foreground hover:text-foreground"
                aria-label={backLabel}
              >
                <ChevronLeft className="size-4" />
                {backLabel}
              </Button>
              <span className="text-border">·</span>
              {ownerRepo ? (
                <>
                  <span className="truncate">
                    <span className="text-muted-foreground">{ownerRepo.owner}</span>
                    <span className="mx-1 text-muted-foreground/60">/</span>
                    <span className="font-medium text-foreground">{ownerRepo.repo}</span>
                  </span>
                  <span className="text-muted-foreground/60">·</span>
                </>
              ) : null}
              <span className="font-mono text-muted-foreground">#{workItem.number}</span>
              <div className="ml-auto flex items-center gap-1">
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      ref={setLinkCopyButtonRef}
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      onClick={() => void handleCopyWorkItemLink()}
                      aria-label="Copy GitHub link"
                    >
                      {linkCopied ? (
                        <Check className="size-4 text-emerald-500" />
                      ) : (
                        <Copy className="size-4" />
                      )}
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="bottom" sideOffset={6}>
                    {linkCopied ? 'Copied' : 'Copy GitHub link'}
                  </TooltipContent>
                </Tooltip>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      onClick={() => api.shell.openUrl(workItem.url)}
                      aria-label="Open on GitHub"
                    >
                      <ExternalLink className="size-4" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="bottom" sideOffset={6}>
                    Open on GitHub
                  </TooltipContent>
                </Tooltip>
              </div>
            </div>
          </div>

          {/* Row 2: large title block */}
          <div className="flex-none border-b border-border/60 bg-card px-6 py-4">
            <div className="flex items-start gap-4">
              <h1 className="min-w-0 flex-1 text-[28px] font-medium leading-tight text-foreground">
                <span className="break-words">{workItem.title}</span>
                <span className="ml-2 font-light text-muted-foreground">#{workItem.number}</span>
              </h1>
              <div className="flex shrink-0 items-center gap-2">
                {/* Why: Agentum's signature affordance — keep this primary so it
                    stands out against GitHub's familiar surface. */}
                {issueAttachedWorkspace ? (
                  <DropdownMenu modal={false}>
                    <ButtonGroup>
                      <Button
                        type="button"
                        size="sm"
                        onClick={() => handleOpenOrUseIssueWorkspace(workItem)}
                        className="gap-1.5 whitespace-nowrap"
                        aria-label="Open workspace attached to issue"
                      >
                        Open workspace
                        <ArrowRight className="size-3.5" />
                      </Button>
                      <DropdownMenuTrigger asChild>
                        <Button
                          type="button"
                          size="icon-sm"
                          aria-label="More issue workspace actions"
                        >
                          <ChevronDown className="size-3.5" />
                        </Button>
                      </DropdownMenuTrigger>
                    </ButtonGroup>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem onSelect={() => onUse(workItem)}>
                        <Plus className="size-4" />
                        Start new workspace
                      </DropdownMenuItem>
                      <DropdownMenuItem onSelect={() => api.shell.openUrl(workItem.url)}>
                        <ExternalLink className="size-4" />
                        Open on GitHub
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                ) : (
                  <Button
                    type="button"
                    size="sm"
                    onClick={() => onUse(workItem)}
                    className="gap-1.5 whitespace-nowrap"
                    aria-label="Start workspace from issue"
                  >
                    Start workspace from issue
                    <ArrowRight className="size-3.5" />
                  </Button>
                )}
              </div>
            </div>
            <div className="mt-3 flex flex-wrap items-center gap-2 text-[13px] text-muted-foreground">
              <span
                className={cn(
                  'inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-[12px] font-medium',
                  issueStateBadgeTone
                )}
              >
                {localState === 'closed' ? (
                  <CircleDashed className="size-3.5" />
                ) : (
                  <CircleDot className="size-3.5" />
                )}
                {localState === 'closed' ? 'Closed' : 'Open'}
              </span>
              <span className="flex flex-wrap items-center gap-1.5">
                <span className="font-semibold text-foreground">
                  {workItem.author ?? 'unknown'}
                </span>
                <span>opened this issue</span>
                <span className="text-muted-foreground/80">
                  · updated {formatRelativeTime(workItem.updatedAt)}
                </span>
              </span>
              <WorkItemIssueSourceIndicator url={workItem.url} repoId={effectiveRepoId} />
              {issueAttachedWorkspaceLabel ? (
                <span className="inline-flex min-w-0 items-center gap-1.5">
                  <FolderKanban className="size-3.5 shrink-0" />
                  <span className="truncate">{issueAttachedWorkspaceLabel}</span>
                </span>
              ) : null}
            </div>
          </div>
        </>
      ) : (
        <div className="flex-none border-b border-border/60 bg-card/80 px-4 py-3 shadow-xs backdrop-blur supports-[backdrop-filter]:bg-card/70">
          <div className="flex items-start gap-3">
            {variant === 'page' ? (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={onClose}
                className="-ml-1 mt-0.5 shrink-0 gap-1.5"
                aria-label={backLabel}
              >
                <ChevronLeft className="size-4" />
                {backLabel}
              </Button>
            ) : null}
            <div className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-md border border-border/60 bg-muted/40 text-muted-foreground">
              <Icon className="size-4" />
            </div>
            <div className="min-w-0 flex-1 space-y-1">
              <div className="flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
                <WorkItemStateBadge item={{ ...workItem, state: localState }} />
                <span className="font-mono">#{workItem.number}</span>
                <span>{workItem.type === 'pr' ? 'Pull request' : 'Issue'}</span>
              </div>
              <h2 className="text-[15px] font-semibold leading-snug text-foreground">
                {workItem.title}
              </h2>
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px] text-muted-foreground">
                <span>{workItem.author ?? 'unknown'}</span>
                <span>updated {formatRelativeTime(workItem.updatedAt)}</span>
                {workItem.branchName && (
                  <span className="max-w-full truncate rounded-md border border-border/50 bg-muted/40 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
                    {workItem.branchName}
                  </span>
                )}
                {issueAttachedWorkspaceLabel ? (
                  <span className="inline-flex min-w-0 items-center gap-1">
                    <FolderKanban className="size-3 shrink-0" />
                    <span className="truncate">{issueAttachedWorkspaceLabel}</span>
                  </span>
                ) : null}
              </div>
              {workItem.type === 'issue' && (
                <WorkItemIssueSourceIndicator url={workItem.url} repoId={effectiveRepoId} />
              )}
            </div>
            <div className="flex shrink-0 items-center justify-end gap-1">
              {workItem.type === 'pr' && (
                <Button
                  type="button"
                  size="sm"
                  onClick={() => onUse(workItem)}
                  className="gap-1.5 whitespace-nowrap"
                  aria-label="Start workspace from PR"
                >
                  Start workspace from PR
                  <ArrowRight className="size-3.5" />
                </Button>
              )}
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    ref={setLinkCopyButtonRef}
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    onClick={() => void handleCopyWorkItemLink()}
                    aria-label="Copy GitHub link"
                  >
                    {linkCopied ? (
                      <Check className="size-4 text-emerald-500" />
                    ) : (
                      <Copy className="size-4" />
                    )}
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom" sideOffset={6}>
                  {linkCopied ? 'Copied' : 'Copy GitHub link'}
                </TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    onClick={() => api.shell.openUrl(workItem.url)}
                    aria-label="Open on GitHub"
                  >
                    <ExternalLink className="size-4" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom" sideOffset={6}>
                  Open on GitHub
                </TooltipContent>
              </Tooltip>
              {variant === 'sheet' ? (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      onClick={onClose}
                      aria-label="Close preview"
                    >
                      <X className="size-4" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="bottom" sideOffset={6}>
                    Close · Esc
                  </TooltipContent>
                </Tooltip>
              ) : null}
            </div>
          </div>
        </div>
      )}

      {!isIssuePage && (repoPath || projectOrigin) && (
        <GHEditSection
          item={workItem}
          repoPath={repoPath}
          repoId={effectiveRepoId}
          projectOrigin={projectOrigin}
          localState={localState}
          localLabels={localLabels}
          onStateChange={setLocalState}
          onLabelsChange={setLocalLabels}
          onMutated={() => {
            // Why: drop the cached details for this item so the next
            // open issues a fresh fetch instead of painting pre-edit
            // state. We invalidate by (repoPath, type, number) match
            // because a single mutation can affect entries across all
            // issueSourcePreference values for the same number.
            if (repoPath) {
              invalidateWorkItemDetailsCacheByMatch({
                repoPath,
                repoId: effectiveRepoId ?? undefined,
                type: workItem.type,
                number: workItem.number
              })
            }
          }}
          assignees={details?.assignees ?? []}
          onUse={onUse}
          onOpenOrUse={handleOpenOrUseIssueWorkspace}
          attachedWorkspaceLabel={issueAttachedWorkspaceLabel}
        />
      )}

      <div className="min-h-0 flex-1">
        {error ? (
          <div className="px-4 py-6 text-[12px] text-destructive">{error}</div>
        ) : isIssuePage ? (
          <div className="h-full min-h-0 overflow-y-auto scrollbar-sleek bg-background">
            <div className="mx-auto grid w-full max-w-[1280px] grid-cols-1 gap-8 px-6 py-6 lg:grid-cols-[minmax(0,1fr)_260px]">
              <div className="min-w-0">
                <ConversationTab
                  item={displayWorkItem ?? workItem}
                  repoPath={repoPath}
                  repoId={effectiveRepoId}
                  body={body}
                  comments={comments}
                  files={files}
                  headSha={details?.headSha}
                  baseSha={details?.baseSha}
                  loading={loading}
                  detailsLoaded={detailsLoaded}
                  checks={checks}
                  localState={localState}
                  onStateChange={setLocalState}
                  projectOrigin={projectOrigin}
                  onMutated={() => {
                    if (repoPath) {
                      invalidateWorkItemDetailsCacheByMatch({
                        repoPath,
                        repoId: effectiveRepoId ?? undefined,
                        type: workItem.type,
                        number: workItem.number
                      })
                    }
                  }}
                  onChecksUpdated={(nextChecks) => {
                    if (detailsCacheKey) {
                      patchCachedPRChecks(detailsCacheKey, nextChecks)
                    }
                  }}
                  onBodyUpdated={(nextBody) => {
                    if (detailsCacheKey) {
                      patchCachedWorkItemBody(detailsCacheKey, nextBody)
                    }
                  }}
                  onCommentAdded={appendOptimisticComment}
                  onReviewersRequested={(nextReviewRequests) => {
                    if (detailsCacheKey) {
                      patchCachedPRReviewRequests(detailsCacheKey, nextReviewRequests)
                    }
                    onReviewRequestsChange?.(
                      { id: workItem.id, repoId: workItem.repoId },
                      nextReviewRequests
                    )
                  }}
                />
              </div>
              {(repoPath || projectOrigin) && (
                <div className="min-w-0">
                  <div className="lg:sticky lg:top-4">
                    <GHEditSection
                      item={workItem}
                      repoPath={repoPath}
                      repoId={effectiveRepoId}
                      projectOrigin={projectOrigin}
                      localState={localState}
                      localLabels={localLabels}
                      onStateChange={setLocalState}
                      onLabelsChange={setLocalLabels}
                      onMutated={() => {
                        if (repoPath) {
                          invalidateWorkItemDetailsCacheByMatch({
                            repoPath,
                            repoId: effectiveRepoId ?? undefined,
                            type: workItem.type,
                            number: workItem.number
                          })
                        }
                      }}
                      assignees={details?.assignees ?? []}
                      onUse={onUse}
                      onOpenOrUse={handleOpenOrUseIssueWorkspace}
                      attachedWorkspaceLabel={issueAttachedWorkspaceLabel}
                      layout="sidebar"
                    />
                  </div>
                </div>
              )}
            </div>
          </div>
        ) : (
          <Tabs
            value={tab}
            onValueChange={(value) => setTab(value as ItemDialogTab)}
            className="flex h-full min-h-0 flex-col gap-0"
          >
            <TabsList
              variant="line"
              className="mx-4 mt-2 justify-start gap-3 border-b border-border/60 bg-transparent"
            >
              <TabsTrigger value="conversation" className="px-2">
                <MessageSquare className="size-3.5" />
                Conversation
              </TabsTrigger>
              {workItem.type === 'pr' && (
                <>
                  <TabsTrigger value="checks" className="px-2">
                    <ListChecks className="size-3.5" />
                    Checks
                    {checks.length > 0 && (
                      <span className="ml-1 text-[10px] text-muted-foreground">
                        {checks.length}
                      </span>
                    )}
                  </TabsTrigger>
                  <TabsTrigger value="files" className="px-2">
                    <FileText className="size-3.5" />
                    Files
                    {files.length > 0 && (
                      <span className="ml-1 text-[10px] text-muted-foreground">{files.length}</span>
                    )}
                  </TabsTrigger>
                </>
              )}
            </TabsList>

            <div className="min-h-0 flex-1 overflow-y-auto scrollbar-sleek">
              <TabsContent value="conversation" className="mt-0">
                <ConversationTab
                  item={displayWorkItem ?? workItem}
                  repoPath={repoPath}
                  repoId={effectiveRepoId}
                  body={body}
                  comments={comments}
                  files={files}
                  headSha={details?.headSha}
                  baseSha={details?.baseSha}
                  loading={loading}
                  detailsLoaded={detailsLoaded}
                  checks={checks}
                  localState={localState}
                  onStateChange={setLocalState}
                  projectOrigin={projectOrigin}
                  onMutated={() => {
                    if (repoPath) {
                      invalidateWorkItemDetailsCacheByMatch({
                        repoPath,
                        repoId: effectiveRepoId ?? undefined,
                        type: workItem.type,
                        number: workItem.number
                      })
                    }
                  }}
                  onChecksUpdated={(nextChecks) => {
                    if (detailsCacheKey) {
                      patchCachedPRChecks(detailsCacheKey, nextChecks)
                    }
                  }}
                  onBodyUpdated={(nextBody) => {
                    if (detailsCacheKey) {
                      patchCachedWorkItemBody(detailsCacheKey, nextBody)
                    }
                  }}
                  onCommentAdded={appendOptimisticComment}
                  onReviewersRequested={(nextReviewRequests) => {
                    if (detailsCacheKey) {
                      patchCachedPRReviewRequests(detailsCacheKey, nextReviewRequests)
                    }
                    onReviewRequestsChange?.(
                      { id: workItem.id, repoId: workItem.repoId },
                      nextReviewRequests
                    )
                  }}
                />
              </TabsContent>

              {workItem.type === 'pr' && (
                <>
                  <TabsContent value="checks" className="mt-0">
                    <ChecksTab
                      item={workItem}
                      repoPath={repoPath}
                      repoId={effectiveRepoId}
                      headSha={details?.headSha}
                      checks={checks}
                      loading={loading || !detailsLoaded}
                      variant="page"
                      onChecksUpdated={(nextChecks) => {
                        if (detailsCacheKey) {
                          patchCachedPRChecks(detailsCacheKey, nextChecks)
                        }
                      }}
                    />
                  </TabsContent>

                  <TabsContent value="files" className="mt-0">
                    {loading && files.length === 0 ? (
                      <div className="flex items-center justify-center py-10">
                        <LoaderCircle className="size-5 animate-spin text-muted-foreground" />
                      </div>
                    ) : files.length === 0 ? (
                      <div className="px-4 py-10 text-center text-[12px] text-muted-foreground">
                        No files changed.
                      </div>
                    ) : (
                      <PRFilesCombinedDiffViewer
                        files={files}
                        comments={comments}
                        repoPath={repoPath ?? ''}
                        repoId={effectiveRepoId ?? ''}
                        prNumber={workItem.number}
                        prUrl={workItem.url}
                        headSha={details?.headSha}
                        baseSha={details?.baseSha}
                        pendingViewedPaths={pendingViewedPaths}
                        onCommentAdded={appendOptimisticComment}
                        onViewedChange={handlePRFileViewedChange}
                      />
                    )}
                  </TabsContent>
                </>
              )}
            </div>
          </Tabs>
        )}
      </div>
    </div>
  ) : null

  if (variant === 'page') {
    return (
      <div className="flex h-full min-h-0 flex-col overflow-hidden rounded-md border border-border/50 bg-background shadow-sm">
        {content}
      </div>
    )
  }

  return (
    <Sheet open={workItem !== null} onOpenChange={(open) => !open && onClose()}>
      <SheetContent
        side="right"
        showCloseButton={false}
        className={cn(
          'flex w-full flex-col gap-0 overflow-hidden p-0 lg:max-w-[var(--github-item-dialog-max-width)]',
          // Why: native macOS traffic lights are drawn above web content, so a
          // nearly full-width right sheet must leave the titlebar's 80px
          // traffic-light pad uncovered instead of relying on z-index.
          IS_MAC
            ? 'max-w-[calc(100vw-(80px/var(--ui-zoom-factor,1)))] sm:max-w-[calc(100vw-(80px/var(--ui-zoom-factor,1)))]'
            : 'max-w-[calc(100vw-1rem)] sm:max-w-[calc(100vw-1rem)]'
        )}
        style={
          {
            '--github-item-dialog-max-width': IS_MAC
              ? 'min(calc(100vw - (80px / var(--ui-zoom-factor, 1))), 1600px)'
              : 'min(calc(100vw - 2rem), 1600px)'
          } as React.CSSProperties
        }
        onOpenAutoFocus={(event) => {
          // Why: focusing the first actionable element inside the drawer
          // causes the "Start workspace" action to receive focus and
          // get visually highlighted on open. Preventing auto-focus keeps the
          // drawer feeling like a passive preview until the user acts.
          event.preventDefault()
        }}
      >
        {/* Why: SheetTitle/Description are required by Radix Dialog for a11y,
            but the visible header carries the same info. Wrap each with
            `asChild` so the VisuallyHidden span wraps the element cleanly. */}
        <VisuallyHidden.Root asChild>
          <SheetTitle>{workItem?.title ?? 'GitHub item'}</SheetTitle>
        </VisuallyHidden.Root>
        <VisuallyHidden.Root asChild>
          <SheetDescription>
            Preview and edit the selected GitHub issue or pull request.
          </SheetDescription>
        </VisuallyHidden.Root>

        {content}
      </SheetContent>
    </Sheet>
  )
}
