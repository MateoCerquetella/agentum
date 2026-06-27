import { api } from '@/tauri'
import { GHEditSection } from './github-item-edit-section'
import { PRActionsPanel } from './github-item-actions-panel'
import { ChecksTab } from './github-item-checks-tab'
import type { GitHubItemDialogProjectOrigin } from './github-item-types'
import { WorkItemIssueSourceIndicator } from './github-item-issue-source-indicator'
import { GHCommentComposer } from './github-item-comment-composer'
import { CommentReplyForm } from './github-item-comment-reply-form'
import { PRReviewersPanel } from './github-item-reviewers-panel'
import { parseOwnerRepoFromItemUrl } from '@/lib/github-item-url'
import { findNearestBraceBlock, getPRFileContentCacheKey, getPRFileDiffResult, getPRFileSectionKey, getWorkItemDetailsCacheKey, gitHubPRFileToBranchEntry, isPRFileViewed } from '@/lib/github-pr-detail-helpers'
import { normalizeItemDialogTab } from '@/lib/github-work-item-state'
import type { ItemDialogTab } from '@/shared/types'
export type { ItemDialogTab }
import {
  addIssueCommentForRepo,
  addPRReviewCommentForRepo,
  addPRReviewCommentReplyForRepo,
  getWorkItemDetailsForRepo,
  setPRFileViewedForRepo
} from '@/lib/github-repo-operations'
import { CommentReactions, PRViewedCheckbox, WorkItemStateBadge } from './github-item-display'
import { formatRelativeTime } from '@/lib/relative-time'
/* eslint-disable max-lines -- Why: the GH item dialog keeps its header, conversation, files, and checks tabs co-located so the read-only PR/Issue surface stays in one place while this view evolves. */
import React, {
  Suspense,
  lazy,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore
} from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { editor as monacoEditor } from 'monaco-editor'
import { ArrowDown, ArrowRight, ArrowUp, Braces, Check, ChevronDown, ChevronLeft, CircleDashed, CircleDot, Copy, ExternalLink, FileText, FolderKanban, GitPullRequest, ListChecks, LoaderCircle, MessageSquare, MessageSquarePlus, PanelLeftOpen, Pencil, Plus, UndoDot, X } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { ButtonGroup } from '@/components/ui/button-group'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { VisuallyHidden } from 'radix-ui'
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger
} from '@/components/ui/accordion'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import CommentMarkdown from '@/components/sidebar/CommentMarkdown'
import { detectLanguage } from '@/lib/language-detect'
import { cn } from '@/lib/utils'
import { DiffSectionItem } from '@/components/editor/DiffSectionItem'
import type { DecoratedDiffComment } from '@/components/diff-comments/useDiffCommentDecorator'
import {
  CombinedDiffFileTree,
  createCombinedDiffSectionIndexMap,
  handleCombinedDiffFileTreeNavigation
} from '@/components/editor/CombinedDiffFileTree'
import {
  getDiffSectionEstimatedHeight,
  isIntrinsicHeightImageDiff
} from '@/components/editor/diff-section-layout'
import type { DiffSection } from '@/components/editor/diff-section-types'
import type { CombinedDiffFileTreeEntry } from '@/components/editor/combined-diff-file-tree-model'
import {
  clearGitHubLinkCopied,
  createGitHubLinkCopyState,
  markGitHubLinkCopied,
  resolveGitHubLinkCopyState
} from '@/components/github-link-copy-state'
import {
  filterPRCommentsByAudience,
  getPRCommentAudienceCounts,
  getPRCommentAudienceEmptyLabel,
  PR_COMMENT_AUDIENCE_FILTERS,
  type PRCommentAudienceFilter
} from '@/lib/pr-comment-audience'
import {
  getPRCommentGroupCount,
  getPRCommentGroupId,
  getPRCommentGroupRoot,
  groupPRComments,
  isResolvedPRCommentGroup,
  PR_COMMENT_OPEN_AUTHOR_CLASS,
  PR_COMMENT_RESOLVED_AUTHOR_CLASS,
  PR_COMMENT_RESOLVED_CONTAINER_CLASS,
  type PRCommentGroup
} from '@/lib/pr-comment-groups'
import {
  createCommentCodeContextExpansionState,
  resolveCommentCodeContextExpansionState,
  updateCommentCodeContextExpansionState,
  type CommentCodeContextLineUpdate
} from '@/components/comment-code-context-state'
import { resolveCommentReplyTarget } from '@/components/comment-reply-target-state'
import { useAppStore } from '@/store'
import { useAllWorktrees } from '@/store/selectors'
import { callRuntimeRpc, getActiveRuntimeTarget } from '@/runtime/runtime-rpc-client'
import { useImmediateMutation } from '@/hooks/useIssueMetadata'
import { GitHubMarkdownComposer } from '@/components/github/GitHubMarkdownComposer'
import { findGithubIssueWorkspaceAttachment, getGithubWorkItemWorkspaceAttachmentLabel } from '@/lib/github-work-item-workspace-attachment'
import { activateAndRevealWorktree } from '@/lib/worktree-activation'
import type { GitHubOwnerRepo, GitHubPRFile, GitHubPRFileContents, GitHubPRFileViewedState, GitHubWorkItem, GitHubWorkItemDetails, GitHubAssignableUser, GitBranchChangeEntry, GitDiffResult, PRCheckDetail, PRComment } from '../../../shared/types'

const IS_MAC = navigator.userAgent.includes('Mac')

const MonacoCodeExcerpt = lazy(() => import('@/components/editor/MonacoCodeExcerpt'))

const CODE_CONTEXT_EXPAND_STEP = 5
const CODE_CONTEXT_FALLBACK_LINES = 20
const CODE_CONTEXT_MAX_BLOCK_LINES = CODE_CONTEXT_FALLBACK_LINES * 2 + 1

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

// Why: bounded LRU — opening many PRs with many files during a session
// would otherwise grow this module-level map without bound until reload.
const PR_FILE_CONTENT_CACHE_MAX = 64
const prFileContentCache = new Map<string, Promise<GitHubPRFileContents> | GitHubPRFileContents>()

function touchPRFileContentCache(
  key: string,
  value: Promise<GitHubPRFileContents> | GitHubPRFileContents
): void {
  // Why: re-insert to move to the most-recently-used position; Map preserves
  // insertion order so the oldest key is always first when evicting.
  prFileContentCache.delete(key)
  prFileContentCache.set(key, value)
  while (prFileContentCache.size > PR_FILE_CONTENT_CACHE_MAX) {
    const oldest = prFileContentCache.keys().next().value
    if (oldest === undefined) {
      break
    }
    prFileContentCache.delete(oldest)
  }
}

function loadPRFileContents(args: {
  repoPath: string
  repoId: string
  prNumber: number
  file: GitHubPRFile
  headSha: string
  baseSha: string
}): Promise<GitHubPRFileContents> {
  const cacheKey = getPRFileContentCacheKey(args)
  const cached = prFileContentCache.get(cacheKey)
  if (cached) {
    touchPRFileContentCache(cacheKey, cached)
    return Promise.resolve(cached)
  }
  const request = api.gh
    .prFileContents({
      repoPath: args.repoPath,
      repoId: args.repoId,
      prNumber: args.prNumber,
      path: args.file.path,
      oldPath: args.file.oldPath,
      status: args.file.status,
      headSha: args.headSha,
      baseSha: args.baseSha
    })
    .then((contents) => {
      touchPRFileContentCache(cacheKey, contents)
      return contents
    })
    .catch((err) => {
      prFileContentCache.delete(cacheKey)
      throw err
    })
  touchPRFileContentCache(cacheKey, request)
  return request
}

const PR_DIFF_OVERSCAN = 5

type PRFilesCombinedDiffViewerProps = {
  files: GitHubPRFile[]
  comments: PRComment[]
  repoPath: string
  repoId: string
  prNumber: number
  prUrl: string
  headSha: string | undefined
  baseSha: string | undefined
  pendingViewedPaths: ReadonlySet<string>
  onCommentAdded: (comment: PRComment) => void
  onViewedChange: (path: string, viewed: boolean) => Promise<boolean>
}

function PRFilesCombinedDiffViewer({
  files,
  comments,
  repoPath,
  repoId,
  prNumber,
  prUrl,
  headSha,
  baseSha,
  pendingViewedPaths,
  onCommentAdded,
  onViewedChange
}: PRFilesCombinedDiffViewerProps): React.JSX.Element {
  const settings = useAppStore((s) => s.settings)
  const isDark =
    settings?.theme === 'dark' ||
    (settings?.theme === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)
  const entriesCacheRef = useRef<{
    signature: string
    entries: GitBranchChangeEntry[]
  } | null>(null)
  const diffEntrySignature = useMemo(
    () =>
      JSON.stringify(
        files.map((file) => ({
          path: file.path,
          oldPath: file.oldPath ?? null,
          status: file.status,
          additions: file.additions,
          deletions: file.deletions,
          isBinary: file.isBinary
        }))
      ),
    [files]
  )
  const entries = useMemo(() => {
    if (entriesCacheRef.current?.signature === diffEntrySignature) {
      return entriesCacheRef.current.entries
    }
    const nextEntries = files.map(gitHubPRFileToBranchEntry)
    entriesCacheRef.current = { signature: diffEntrySignature, entries: nextEntries }
    return nextEntries
  }, [diffEntrySignature, files])
  const fileByPath = useMemo(() => new Map(files.map((file) => [file.path, file])), [files])
  const inlineReviewComments = useMemo<DecoratedDiffComment[]>(
    () =>
      comments.flatMap((comment): DecoratedDiffComment[] => {
        // Why: stale threads keep originalLine for the sidebar, but rendering
        // that number inline can attach the comment to unrelated current code.
        if (comment.isOutdated || !comment.path || typeof comment.line !== 'number') {
          return []
        }
        const createdAtMs = new Date(comment.createdAt).getTime()
        return [
          {
            id: `github-pr-comment:${comment.id}`,
            worktreeId: `github-pr:${repoId}:${prNumber}`,
            filePath: comment.path,
            source: 'diff',
            startLine: comment.startLine,
            lineNumber: comment.line,
            body: comment.body,
            createdAt: Number.isFinite(createdAtMs) ? createdAtMs : Date.now(),
            side: 'modified',
            author: comment.author,
            authorAvatarUrl: comment.authorAvatarUrl,
            createdAtLabel: formatRelativeTime(comment.createdAt),
            url: comment.url,
            canDelete: false,
            canEdit: false
          }
        ]
      }),
    [comments, prNumber, repoId]
  )
  const entrySignature = useMemo(
    () =>
      JSON.stringify({
        repoId,
        prNumber,
        headSha: headSha ?? null,
        baseSha: baseSha ?? null,
        files: diffEntrySignature
      }),
    [baseSha, diffEntrySignature, headSha, prNumber, repoId]
  )
  const [sections, setSections] = useState<DiffSection[]>([])
  const [sideBySide, setSideBySide] = useState(false)
  const [fileTreeCollapsed, setFileTreeCollapsed] = useState(false)
  const [sectionHeights, setSectionHeights] = useState<Record<number, number>>({})
  const [activeTreeSectionKey, setActiveTreeSectionKey] = useState<string | null>(null)
  const scrollContainerRef = useRef<HTMLDivElement>(null)
  const loadedIndicesRef = useRef<Set<number>>(new Set())
  const loadingIndicesRef = useRef<Set<number>>(new Set())
  const sectionsRef = useRef<DiffSection[]>([])
  const generationRef = useRef(0)
  const modifiedEditorsRef = useRef<Map<number, monacoEditor.IStandaloneCodeEditor>>(new Map())
  const handleSectionSaveRef = useRef<(index: number) => Promise<void>>(async () => {})
  sectionsRef.current = sections

  useEffect(() => {
    generationRef.current += 1
    loadedIndicesRef.current.clear()
    loadingIndicesRef.current.clear()
    setSectionHeights({})
    setActiveTreeSectionKey(null)
    setSections(
      entries.map((entry) => ({
        key: getPRFileSectionKey(entry.path),
        path: entry.path,
        oldPath: entry.oldPath,
        status: entry.status,
        added: entry.added,
        removed: entry.removed,
        originalContent: '',
        modifiedContent: '',
        collapsed: false,
        loading: true,
        error: undefined,
        dirty: false,
        diffResult: null
      }))
    )
  }, [entries, entrySignature])

  const loadSection = useCallback(
    (index: number) => {
      const section = sectionsRef.current[index]
      if (!section || section.collapsed) {
        return
      }
      if (loadedIndicesRef.current.has(index) || loadingIndicesRef.current.has(index)) {
        return
      }
      const file = fileByPath.get(section.path)
      if (!file) {
        return
      }
      const generation = generationRef.current
      loadingIndicesRef.current.add(index)

      const load = async (): Promise<{ result: GitDiffResult; error?: string }> => {
        if (file.isBinary) {
          return {
            result: {
              kind: 'binary',
              originalContent: '',
              modifiedContent: '',
              originalIsBinary: true,
              modifiedIsBinary: true
            }
          }
        }
        if (!headSha || !baseSha) {
          return {
            result: {
              kind: 'text',
              originalContent: '',
              modifiedContent: '',
              originalIsBinary: false,
              modifiedIsBinary: false
            },
            error: 'Diff unavailable because the PR commit SHAs are missing.'
          }
        }
        const contents = await loadPRFileContents({
          repoPath,
          repoId,
          prNumber,
          file,
          headSha,
          baseSha
        })
        return { result: getPRFileDiffResult(contents) }
      }

      load()
        .catch((error) => ({
          result: {
            kind: 'text',
            originalContent: '',
            modifiedContent: '',
            originalIsBinary: false,
            modifiedIsBinary: false
          } as GitDiffResult,
          error: error instanceof Error ? error.message : 'Failed to load diff.'
        }))
        .then(({ result, error }) => {
          loadingIndicesRef.current.delete(index)
          if (generationRef.current !== generation) {
            return
          }
          loadedIndicesRef.current.add(index)
          setSections((prev) =>
            prev.map((current, currentIndex) =>
              currentIndex === index
                ? {
                    ...current,
                    diffResult: result,
                    originalContent: result.kind === 'text' ? result.originalContent : '',
                    modifiedContent: result.kind === 'text' ? result.modifiedContent : '',
                    loading: false,
                    error
                  }
                : current
            )
          )
        })
    },
    [baseSha, fileByPath, headSha, prNumber, repoId, repoPath]
  )

  const retrySection = useCallback(
    (index: number) => {
      loadedIndicesRef.current.delete(index)
      loadingIndicesRef.current.delete(index)
      setSections((prev) =>
        prev.map((section, sectionIndex) =>
          sectionIndex === index
            ? {
                ...section,
                diffResult: null,
                originalContent: '',
                modifiedContent: '',
                loading: true,
                error: undefined
              }
            : section
        )
      )
      loadSection(index)
    },
    [loadSection]
  )

  const toggleSection = useCallback(
    (index: number) => {
      const shouldLoadAfterExpand = sectionsRef.current[index]?.collapsed ?? false
      setSections((prev) =>
        prev.map((section, sectionIndex) =>
          sectionIndex === index ? { ...section, collapsed: !section.collapsed } : section
        )
      )
      if (shouldLoadAfterExpand) {
        window.requestAnimationFrame(() => loadSection(index))
      }
    },
    [loadSection]
  )

  const setAllSectionsCollapsed = useCallback(
    (collapsed: boolean) => {
      setSections((prev) => prev.map((section) => ({ ...section, collapsed })))
      if (!collapsed) {
        window.requestAnimationFrame(() => {
          sectionsRef.current.forEach((_, index) => loadSection(index))
        })
      }
    },
    [loadSection]
  )

  const allSectionsCollapsed = sections.length > 0 && sections.every((section) => section.collapsed)
  const sectionIndexByKey = useMemo(() => createCombinedDiffSectionIndexMap(sections), [sections])
  const viewedSectionKeys = useMemo(
    () => new Set(files.filter(isPRFileViewed).map((file) => getPRFileSectionKey(file.path))),
    [files]
  )

  const virtualizer = useVirtualizer({
    count: sections.length,
    getScrollElement: () => scrollContainerRef.current,
    estimateSize: (index) => {
      const section = sections[index]
      if (!section) {
        return 88
      }
      return getDiffSectionEstimatedHeight({
        collapsed: section.collapsed,
        measuredContentHeight: sectionHeights[index],
        originalContent: section.originalContent,
        modifiedContent: section.modifiedContent,
        changedLineCount:
          section.added === undefined && section.removed === undefined
            ? undefined
            : (section.added ?? 0) + (section.removed ?? 0),
        useIntrinsicImageHeight: isIntrinsicHeightImageDiff(section.diffResult)
      })
    },
    overscan: PR_DIFF_OVERSCAN,
    getItemKey: (index) => {
      const section = sections[index]
      return section
        ? `${section.key}:${section.collapsed ? 'collapsed' : 'expanded'}:${entrySignature}`
        : `${index}:${entrySignature}`
    }
  })

  useLayoutEffect(() => {
    virtualizer.measure()
  }, [sideBySide, virtualizer])

  const handleTreeNavigate = useCallback(
    (entry: CombinedDiffFileTreeEntry) => {
      const navigatedIndex = handleCombinedDiffFileTreeNavigation({
        mode: 'commit',
        entry,
        sections: sectionsRef.current,
        sectionIndexByKey,
        toggleSection,
        scrollToIndex: (index) => virtualizer.scrollToIndex(index, { align: 'start' })
      })
      if (navigatedIndex !== null) {
        setActiveTreeSectionKey(sectionsRef.current[navigatedIndex]?.key ?? null)
      }
    },
    [sectionIndexByKey, toggleSection, virtualizer]
  )

  const openFilesOnGitHub = useCallback(() => {
    void api.shell.openUrl(`${prUrl.replace(/\/$/, '')}/files`)
  }, [prUrl])

  const handleAddLineComment = useCallback(
    async (
      section: DiffSection,
      {
        lineNumber,
        startLine,
        body
      }: {
        lineNumber: number
        startLine?: number
        body: string
      }
    ) => {
      if (!headSha) {
        toast.error('Unable to comment without the PR head SHA.')
        return false
      }
      const result = await addPRReviewCommentForRepo({
        repoPath,
        repoId,
        prNumber,
        commitId: headSha,
        path: section.path,
        line: lineNumber,
        startLine,
        body
      })
      if (!result.ok) {
        toast.error(result.error || 'Failed to add review comment.')
        return false
      }
      onCommentAdded(result.comment)
      toast.success('Review comment added.')
      return true
    },
    [headSha, onCommentAdded, prNumber, repoId, repoPath]
  )

  const renderViewedCheckbox = useCallback(
    (section: DiffSection) => {
      const file = fileByPath.get(section.path)
      if (!file) {
        return null
      }
      const viewed = isPRFileViewed(file)
      const pending = pendingViewedPaths.has(file.path)
      return (
        <PRViewedCheckbox
          checked={viewed}
          pending={pending}
          filePath={file.path}
          onToggle={() => {
            if (!pending) {
              void onViewedChange(file.path, !viewed)
            }
          }}
        />
      )
    },
    [fileByPath, onViewedChange, pendingViewedPaths]
  )

  return (
    <div className="flex min-h-[520px] flex-1 flex-col">
      <div className="flex shrink-0 items-center justify-between gap-3 border-b border-border bg-background/50 px-3 py-1.5">
        <div className="flex min-w-0 items-center gap-2">
          {fileTreeCollapsed && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  aria-label="Show file tree"
                  onClick={() => setFileTreeCollapsed(false)}
                >
                  <PanelLeftOpen className="size-3.5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom" sideOffset={6}>
                Show file tree
              </TooltipContent>
            </Tooltip>
          )}
          <span className="truncate text-xs text-muted-foreground">
            {files.filter(isPRFileViewed).length} / {files.length} files viewed
          </span>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <button
            type="button"
            className="w-20 text-left text-xs text-muted-foreground transition-colors hover:text-foreground"
            onClick={() => setAllSectionsCollapsed(!allSectionsCollapsed)}
          >
            {allSectionsCollapsed ? 'Expand All' : 'Collapse All'}
          </button>
          <button
            type="button"
            className="w-24 rounded border border-border px-2 py-0.5 text-center text-xs text-muted-foreground transition-colors hover:text-foreground"
            onClick={() => setSideBySide((prev) => !prev)}
          >
            {sideBySide ? 'Inline' : 'Side by Side'}
          </button>
        </div>
      </div>
      <div className="flex min-h-0 flex-1">
        <CombinedDiffFileTree
          mode="commit"
          worktreePath={repoPath}
          entries={entries}
          sectionIndexByKey={sectionIndexByKey}
          activeSectionKey={activeTreeSectionKey}
          viewedSectionKeys={viewedSectionKeys}
          collapsed={fileTreeCollapsed}
          onCollapsedChange={setFileTreeCollapsed}
          onNavigate={handleTreeNavigate}
        />
        <div ref={scrollContainerRef} className="min-w-0 flex-1 overflow-auto scrollbar-editor">
          <div className="relative w-full" style={{ height: `${virtualizer.getTotalSize()}px` }}>
            {virtualizer.getVirtualItems().map((virtualItem) => {
              const section = sections[virtualItem.index]
              if (!section) {
                return null
              }
              return (
                <div
                  key={virtualItem.key}
                  data-index={virtualItem.index}
                  ref={virtualizer.measureElement}
                  className="absolute left-0 top-0 w-full"
                  style={{ top: `${virtualItem.start}px` }}
                >
                  <DiffSectionItem
                    section={section}
                    index={virtualItem.index}
                    isBranchMode={false}
                    sideBySide={sideBySide}
                    isDark={isDark}
                    settings={settings}
                    sectionHeight={sectionHeights[virtualItem.index]}
                    worktreeId={`github-pr:${repoId}:${prNumber}`}
                    inlineComments={inlineReviewComments}
                    loadSection={loadSection}
                    retrySection={retrySection}
                    toggleSection={toggleSection}
                    openSection={openFilesOnGitHub}
                    openSectionTitle="Open files on GitHub"
                    renderHeaderTrailingContent={renderViewedCheckbox}
                    onAddLineComment={handleAddLineComment}
                    addLineCommentLabel="Comment"
                    addLineCommentPlaceholder="Add a review comment"
                    getCommentableLineNumbers={(section) =>
                      fileByPath.get(section.path)?.reviewCommentLineNumbers
                    }
                    setSectionHeights={setSectionHeights}
                    setSections={setSections}
                    modifiedEditorsRef={modifiedEditorsRef}
                    handleSectionSaveRef={handleSectionSaveRef}
                  />
                </div>
              )
            })}
          </div>
        </div>
      </div>
    </div>
  )
}

function CommentCodeContext({
  comment,
  repoPath,
  repoId,
  prNumber,
  files,
  headSha,
  baseSha
}: {
  comment: PRComment
  repoPath: string | null
  repoId: string
  prNumber: number
  files: GitHubPRFile[]
  headSha: string | undefined
  baseSha: string | undefined
}): React.JSX.Element | null {
  const [contents, setContents] = useState<GitHubPRFileContents | null>(null)
  const [error, setError] = useState(false)
  const [contextExpansionState, setContextExpansionState] = useState(() =>
    createCommentCodeContextExpansionState(comment.id)
  )
  const file = useMemo(
    () => files.find((candidate) => candidate.path === comment.path),
    [comment.path, files]
  )
  const line = comment.line
  const startLine = comment.startLine ?? line

  useEffect(() => {
    setContents(null)
    setError(false)
    if (!repoPath || !file || !headSha || !baseSha || !line || file.isBinary) {
      return
    }
    let cancelled = false
    loadPRFileContents({ repoPath, repoId, prNumber, file, headSha, baseSha })
      .then((result) => {
        if (!cancelled) {
          setContents(result)
        }
      })
      .catch(() => {
        if (!cancelled) {
          setError(true)
        }
      })
    return () => {
      cancelled = true
    }
  }, [baseSha, file, headSha, line, prNumber, repoId, repoPath])

  const resolvedContextExpansionState = resolveCommentCodeContextExpansionState(
    contextExpansionState,
    comment.id
  )
  if (resolvedContextExpansionState !== contextExpansionState) {
    // Why: comment rows can be reused when a PR refreshes; reset before paint
    // so expanded context from the previous comment is never shown on the next.
    setContextExpansionState(resolvedContextExpansionState)
  }
  const contextBefore = resolvedContextExpansionState.contextBefore
  const contextAfter = resolvedContextExpansionState.contextAfter
  const setContextBefore = useCallback(
    (contextBeforeUpdate: CommentCodeContextLineUpdate) => {
      setContextExpansionState((current) =>
        updateCommentCodeContextExpansionState(current, comment.id, {
          contextBefore: contextBeforeUpdate
        })
      )
    },
    [comment.id]
  )
  const setContextAfter = useCallback(
    (contextAfterUpdate: CommentCodeContextLineUpdate) => {
      setContextExpansionState((current) =>
        updateCommentCodeContextExpansionState(current, comment.id, {
          contextAfter: contextAfterUpdate
        })
      )
    },
    [comment.id]
  )

  if (!comment.path || !line || !file || file.isBinary || error) {
    return null
  }

  if (!contents) {
    return (
      <div className="mb-3 flex items-center gap-2 rounded-md border border-border/40 bg-muted/20 px-3 py-2 text-[12px] text-muted-foreground">
        <LoaderCircle className="size-3.5 animate-spin" />
        Loading code context…
      </div>
    )
  }

  const source = contents.modified || contents.original
  const lines = source.split(/\r?\n/)
  const language = detectLanguage(comment.path)
  const commentFrom = Math.max(1, Math.min(startLine ?? line, line))
  const commentTo = Math.min(lines.length, Math.max(startLine ?? line, line))
  const from = Math.max(1, commentFrom - contextBefore)
  const to = Math.min(lines.length, commentTo + contextAfter)
  const selectedLines = lines.slice(from - 1, to)
  const candidateBlockRange = findNearestBraceBlock(lines, commentFrom)
  const candidateBlockLineCount = candidateBlockRange
    ? candidateBlockRange.endLine - candidateBlockRange.startLine + 1
    : 0
  const isWholeFileBlock =
    candidateBlockRange !== null &&
    candidateBlockRange.startLine <= 2 &&
    candidateBlockRange.endLine >= lines.length - 1
  const shouldUseBlockRange =
    candidateBlockRange !== null &&
    !isWholeFileBlock &&
    candidateBlockLineCount <= CODE_CONTEXT_MAX_BLOCK_LINES
  const blockRange = shouldUseBlockRange
    ? candidateBlockRange
    : {
        startLine: Math.max(1, commentFrom - CODE_CONTEXT_FALLBACK_LINES),
        endLine: Math.min(lines.length, commentTo + CODE_CONTEXT_FALLBACK_LINES)
      }
  const canExpandAbove = from > 1
  const canExpandBelow = to < lines.length
  const canExpandBlock = blockRange.startLine < from || blockRange.endLine > to
  const blockTooltip = shouldUseBlockRange
    ? 'Show surrounding code block'
    : 'Show nearby code context'

  if (selectedLines.length === 0) {
    return null
  }

  return (
    <div className="mb-3 overflow-hidden rounded-md border border-border/50 bg-muted/20">
      <div className="flex flex-wrap items-center gap-2 border-b border-border/40 px-3 py-1.5 text-[11px] text-muted-foreground">
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <span className="truncate font-mono">{comment.path}</span>
          <span className="shrink-0 font-mono">
            L{from}
            {to !== from ? `-L${to}` : ''}
          </span>
          {(from !== commentFrom || to !== commentTo) && (
            <span className="shrink-0 font-mono text-muted-foreground/70">
              comment L{commentFrom}
              {commentTo !== commentFrom ? `-L${commentTo}` : ''}
            </span>
          )}
        </div>
        <ButtonGroup className="text-muted-foreground" aria-label="Code context controls">
          {(contextBefore > 0 || contextAfter > 0) && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  variant="outline"
                  size="icon-xs"
                  className="size-7 border-border/55 bg-background/35 text-muted-foreground shadow-none hover:bg-accent hover:text-accent-foreground"
                  onClick={() => {
                    setContextBefore(0)
                    setContextAfter(0)
                  }}
                  aria-label="Reset code context"
                >
                  <UndoDot className="size-3.5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>Reset code context</TooltipContent>
            </Tooltip>
          )}
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="outline"
                size="icon-xs"
                className="size-7 border-border/55 bg-background/35 text-muted-foreground shadow-none hover:bg-accent hover:text-accent-foreground"
                disabled={!canExpandAbove}
                onClick={() =>
                  setContextBefore((current) =>
                    Math.min(current + CODE_CONTEXT_EXPAND_STEP, commentFrom - 1)
                  )
                }
                aria-label={`Show ${CODE_CONTEXT_EXPAND_STEP} more lines above`}
              >
                <ArrowUp className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Show more lines above</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="outline"
                size="icon-xs"
                className="size-7 border-border/55 bg-background/35 text-muted-foreground shadow-none hover:bg-accent hover:text-accent-foreground"
                disabled={!canExpandBelow}
                onClick={() =>
                  setContextAfter((current) =>
                    Math.min(current + CODE_CONTEXT_EXPAND_STEP, lines.length - commentTo)
                  )
                }
                aria-label={`Show ${CODE_CONTEXT_EXPAND_STEP} more lines below`}
              >
                <ArrowDown className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Show more lines below</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="outline"
                size="icon-xs"
                className="size-7 border-border/55 bg-background/35 text-muted-foreground shadow-none hover:bg-accent hover:text-accent-foreground"
                disabled={!canExpandBlock}
                onClick={() => {
                  setContextBefore((current) =>
                    Math.max(current, Math.max(0, commentFrom - blockRange.startLine))
                  )
                  setContextAfter((current) =>
                    Math.max(current, Math.max(0, blockRange.endLine - commentTo))
                  )
                }}
                aria-label={blockTooltip}
              >
                <Braces className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>{blockTooltip}</TooltipContent>
          </Tooltip>
        </ButtonGroup>
      </div>
      <Suspense
        fallback={
          <pre className="overflow-x-auto py-1 text-[12px] leading-5">
            {selectedLines.map((codeLine, index) => {
              const lineNumber = from + index
              const isCommentedLine = lineNumber >= commentFrom && lineNumber <= commentTo
              return (
                <div
                  key={lineNumber}
                  className={cn('flex font-mono', isCommentedLine && 'bg-emerald-500/10')}
                >
                  <span className="w-12 shrink-0 select-none border-r border-border/40 px-2 text-right text-muted-foreground">
                    {lineNumber}
                  </span>
                  <code className="min-w-0 flex-1 px-3 text-foreground">{codeLine || ' '}</code>
                </div>
              )
            })}
          </pre>
        }
      >
        <MonacoCodeExcerpt
          lines={selectedLines}
          firstLineNumber={from}
          highlightedStartLine={commentFrom}
          highlightedEndLine={commentTo}
          language={language}
        />
      </Suspense>
    </div>
  )
}

function ConversationTab({
  item,
  repoPath,
  body,
  comments,
  files,
  headSha,
  baseSha,
  loading,
  detailsLoaded,
  checks,
  localState,
  onStateChange,
  projectOrigin,
  onMutated,
  onChecksUpdated,
  onBodyUpdated,
  onCommentAdded,
  onReviewersRequested
}: {
  item: GitHubWorkItem
  repoPath: string | null
  repoId: string | null
  body: string
  comments: PRComment[]
  files: GitHubPRFile[]
  headSha: string | undefined
  baseSha: string | undefined
  loading: boolean
  detailsLoaded: boolean
  checks: GitHubWorkItemDetails['checks']
  localState: GitHubWorkItem['state']
  onStateChange: (state: GitHubWorkItem['state']) => void
  projectOrigin: GitHubItemDialogProjectOrigin | undefined
  onMutated: () => void
  onChecksUpdated: (checks: PRCheckDetail[]) => void
  onBodyUpdated: (body: string) => void
  onCommentAdded: (comment: PRComment) => void
  onReviewersRequested: (reviewRequests: GitHubAssignableUser[]) => void
}): React.JSX.Element {
  const authorLabel = item.author ?? 'unknown'
  const [replyingTo, setReplyingTo] = useState<number | null>(null)
  const [commentFilter, setCommentFilter] = useState<PRCommentAudienceFilter>('all')
  const [bodyDraft, setBodyDraft] = useState(body)
  const [bodyEditing, setBodyEditing] = useState(false)
  const [bodySaving, setBodySaving] = useState(false)
  const commentCounts = useMemo(() => getPRCommentAudienceCounts(comments), [comments])
  const visibleComments = useMemo(
    () => filterPRCommentsByAudience(comments, commentFilter),
    [commentFilter, comments]
  )
  const visibleCommentGroups = useMemo(() => groupPRComments(visibleComments), [visibleComments])
  const resolvedReplyingTo = resolveCommentReplyTarget(replyingTo, visibleComments)

  if (resolvedReplyingTo !== replyingTo) {
    // Why: comment filters/refetches can hide the active reply target; clear it
    // before paint so a stale composer does not flash for the wrong comment set.
    setReplyingTo(resolvedReplyingTo)
  }

  useEffect(() => {
    if (!bodyEditing) {
      setBodyDraft(body)
    }
  }, [body, bodyEditing, item.id])

  const bodySlug = useMemo(() => parseOwnerRepoFromItemUrl(item.url), [item.url])
  const markdownGitHubRepo = useMemo(
    () => (projectOrigin ? { owner: projectOrigin.owner, repo: projectOrigin.repo } : bodySlug),
    [bodySlug, projectOrigin]
  )
  const canEditBody =
    item.type === 'pr' ? Boolean(projectOrigin || bodySlug) : Boolean(projectOrigin || repoPath)
  const bodyChanged = bodyDraft !== body

  const handleSaveBody = useCallback(async (): Promise<void> => {
    if (bodySaving || !bodyChanged) {
      setBodyEditing(false)
      return
    }
    setBodySaving(true)
    try {
      await runWorkItemBodyUpdate({
        item,
        repoPath,
        projectOrigin,
        body: bodyDraft,
        parsedSlug: bodySlug
      })
      onBodyUpdated(bodyDraft)
      setBodyEditing(false)
      toast.success('Description updated.')
    } catch (err) {
      toast.error(err instanceof Error ? err.message : 'Failed to update description.')
    } finally {
      setBodySaving(false)
    }
  }, [bodyChanged, bodyDraft, bodySaving, bodySlug, item, onBodyUpdated, projectOrigin, repoPath])

  const handleReply = useCallback(
    async (comment: PRComment, replyBody: string): Promise<boolean> => {
      if (!repoPath) {
        toast.error('Unable to reply without a repository path.')
        return false
      }
      const result =
        comment.path && item.type === 'pr'
          ? await addPRReviewCommentReplyForRepo({
              repoPath,
              repoId: item.repoId,
              prNumber: item.number,
              commentId: comment.id,
              body: replyBody,
              threadId: comment.threadId,
              path: comment.path,
              line: comment.line
            })
          : await addIssueCommentForRepo({
              repoPath,
              repoId: item.repoId,
              number: item.number,
              body: `@${comment.author} ${replyBody}`,
              type: item.type
            })

      if (!result.ok) {
        toast.error(result.error || 'Failed to post reply.')
        return false
      }
      onCommentAdded(result.comment)
      setReplyingTo(null)
      toast.success('Reply posted.')
      return true
    },
    [item.number, item.repoId, item.type, onCommentAdded, repoPath]
  )

  const rightPanel =
    item.type === 'pr' ? (
      <div className="flex h-fit flex-col gap-3 xl:sticky xl:top-4">
        <PRActionsPanel
          item={item}
          repoPath={repoPath}
          repoId={item.repoId}
          projectOrigin={projectOrigin}
          localState={localState}
          onStateChange={onStateChange}
          onMutated={onMutated}
        />
        <PRReviewersPanel
          item={item}
          loading={loading}
          repoPath={repoPath}
          onReviewersRequested={onReviewersRequested}
        />
        <aside className="overflow-hidden rounded-lg border border-border/50 bg-card/50 shadow-xs">
          <ChecksTab
            item={item}
            repoPath={repoPath}
            repoId={item.repoId}
            headSha={headSha}
            checks={checks}
            loading={loading || !detailsLoaded}
            onChecksUpdated={onChecksUpdated}
          />
        </aside>
      </div>
    ) : null

  const renderCommentCard = (comment: PRComment, isReply = false): React.JSX.Element => (
    <div
      key={comment.id}
      className={cn(
        'min-w-0 overflow-hidden rounded-lg border border-border/40 bg-card/50 shadow-xs',
        isReply && 'ml-6 max-w-[calc(100%-1.5rem)]',
        comment.isResolved && PR_COMMENT_RESOLVED_CONTAINER_CLASS
      )}
    >
      <div className="flex min-w-0 items-center gap-2 border-b border-border/40 px-3 py-2">
        {comment.authorAvatarUrl ? (
          <img
            src={comment.authorAvatarUrl}
            alt={comment.author}
            className="size-5 shrink-0 rounded-full"
          />
        ) : (
          <div className="size-5 shrink-0 rounded-full bg-muted" />
        )}
        <span
          className={cn(
            'min-w-0 truncate text-[13px] font-semibold',
            comment.isResolved ? PR_COMMENT_RESOLVED_AUTHOR_CLASS : PR_COMMENT_OPEN_AUTHOR_CLASS
          )}
        >
          {comment.author}
        </span>
        <span className="shrink-0 text-[12px] text-muted-foreground">
          · {formatRelativeTime(comment.createdAt)}
        </span>
        {comment.path && (
          <span className="min-w-0 truncate font-mono text-[11px] text-muted-foreground/70">
            {comment.path.split('/').pop()}
            {comment.line ? `:L${comment.line}` : ''}
          </span>
        )}
        {comment.isResolved && (
          <span className="rounded-full border border-border/60 bg-muted/40 px-1.5 py-0.5 text-[11px] text-muted-foreground">
            resolved
          </span>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-1">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                className="size-7"
                onClick={() =>
                  setReplyingTo((current) => (current === comment.id ? null : comment.id))
                }
                aria-label="Reply to comment"
              >
                <MessageSquarePlus className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Reply to comment</TooltipContent>
          </Tooltip>
          {comment.url && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  className="size-7"
                  onClick={() => api.shell.openUrl(comment.url)}
                  aria-label="Open comment on GitHub"
                >
                  <ExternalLink className="size-3.5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>Open comment on GitHub</TooltipContent>
            </Tooltip>
          )}
        </div>
      </div>
      <div className="min-w-0 px-3 py-2">
        <CommentCodeContext
          comment={comment}
          repoPath={repoPath}
          repoId={item.repoId}
          prNumber={item.number}
          files={files}
          headSha={headSha}
          baseSha={baseSha}
        />
        <CommentMarkdown
          content={comment.body}
          variant="document"
          githubRepo={markdownGitHubRepo}
          className="min-w-0 max-w-full overflow-hidden break-words text-[13px] leading-relaxed [&_a]:break-all [&_code]:break-words [&_pre]:max-w-full"
        />
        <CommentReactions reactions={comment.reactions} />
        {resolvedReplyingTo === comment.id && (
          <CommentReplyForm
            className="mt-3"
            placeholder={
              comment.path ? 'Reply in this review thread' : `Reply to @${comment.author}`
            }
            onCancel={() => setReplyingTo(null)}
            onSubmit={(replyBody) => handleReply(comment, replyBody)}
          />
        )}
      </div>
    </div>
  )

  const renderCommentGroup = (group: PRCommentGroup): React.JSX.Element => {
    const cards =
      group.kind === 'thread'
        ? [
            renderCommentCard(group.root),
            ...group.replies.map((reply) => renderCommentCard(reply, true))
          ]
        : [renderCommentCard(group.comment)]

    if (!isResolvedPRCommentGroup(group)) {
      return (
        <div key={getPRCommentGroupId(group)} className="flex min-w-0 flex-col gap-3">
          {cards}
        </div>
      )
    }

    const root = getPRCommentGroupRoot(group)
    const count = getPRCommentGroupCount(group)
    return (
      <Accordion key={getPRCommentGroupId(group)} type="single" collapsible>
        <AccordionItem
          value={getPRCommentGroupId(group)}
          className="rounded-lg border border-border/40 bg-card/40"
        >
          <AccordionTrigger className="px-3 py-2 text-[13px] text-muted-foreground hover:bg-accent/30">
            <span className="min-w-0 truncate">
              Resolved {group.kind === 'thread' ? 'thread' : 'comment'} by {root.author}
              {count > 1 ? ` (${count})` : ''}
            </span>
          </AccordionTrigger>
          <AccordionContent className="flex min-w-0 flex-col gap-3 px-3 pb-3 pt-0">
            {cards}
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    )
  }

  return (
    <div
      className={cn(
        'grid min-w-0 gap-5 px-4 py-4',
        // Why: the drawer expands nearly full-width on narrow app windows, so
        // keep PR controls beside the conversation instead of hiding them below
        // long review threads.
        item.type === 'pr' && 'grid-cols-[minmax(0,1fr)_300px]'
      )}
    >
      <div className="flex min-w-0 flex-col gap-4">
        <div className="rounded-lg border border-border/50 bg-card/50 shadow-xs">
          <div className="flex items-center gap-2 border-b border-border/50 px-3 py-2 text-[12px] text-muted-foreground">
            <span className="font-medium text-foreground">{authorLabel}</span>
            <span>updated {formatRelativeTime(item.updatedAt)}</span>
            {canEditBody && !loading && detailsLoaded ? (
              bodyEditing ? (
                <div className="ml-auto flex items-center gap-1">
                  <Button
                    type="button"
                    variant="ghost"
                    size="xs"
                    className="gap-1.5"
                    disabled={bodySaving}
                    onClick={() => {
                      setBodyDraft(body)
                      setBodyEditing(false)
                    }}
                  >
                    <X className="size-3.5" />
                    Cancel
                  </Button>
                  <Button
                    type="button"
                    size="xs"
                    className="gap-1.5"
                    disabled={bodySaving || !bodyChanged}
                    onClick={() => void handleSaveBody()}
                  >
                    {bodySaving ? (
                      <LoaderCircle className="size-3.5 animate-spin" />
                    ) : (
                      <Check className="size-3.5" />
                    )}
                    Save
                  </Button>
                </div>
              ) : (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-xs"
                      className="ml-auto size-7"
                      onClick={() => {
                        setBodyDraft(body)
                        setBodyEditing(true)
                      }}
                      aria-label="Edit description"
                    >
                      <Pencil className="size-3.5" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>Edit description</TooltipContent>
                </Tooltip>
              )
            ) : null}
          </div>
          <div className="px-4 py-4 text-[14px] leading-relaxed text-foreground">
            {loading && !detailsLoaded ? (
              <div className="flex items-center justify-center py-5">
                <LoaderCircle className="size-4 animate-spin text-muted-foreground" />
              </div>
            ) : bodyEditing ? (
              <GitHubMarkdownComposer
                value={bodyDraft}
                onChange={setBodyDraft}
                placeholder="Description"
                disabled={bodySaving}
                autoFocus
                minHeightClassName="min-h-64"
                onSubmitShortcut={() => void handleSaveBody()}
              />
            ) : body.trim() ? (
              <CommentMarkdown
                content={body}
                variant="document"
                githubRepo={markdownGitHubRepo}
                className="min-w-0 max-w-full overflow-hidden break-words text-[14px] leading-relaxed [&_a]:break-all [&_code]:break-words [&_pre]:max-w-full"
              />
            ) : (
              <span className="italic text-muted-foreground">No description provided.</span>
            )}
          </div>
        </div>

        {detailsLoaded ? (
          <>
            <div className="flex items-center gap-2 pt-1">
              <MessageSquare className="size-4 text-muted-foreground" />
              <span className="text-[13px] font-medium text-foreground">Comments</span>
              {comments.length > 0 && (
                <span className="rounded-full border border-border/50 bg-muted/30 px-1.5 py-0.5 text-[11px] tabular-nums text-muted-foreground">
                  {comments.length}
                </span>
              )}
            </div>

            {item.type === 'pr' && comments.length > 0 && (
              <div className="grid grid-cols-3 rounded-lg border border-border/50 bg-background p-0.5">
                {PR_COMMENT_AUDIENCE_FILTERS.map((filter) => {
                  const isActive = commentFilter === filter.value
                  return (
                    <button
                      key={filter.value}
                      type="button"
                      className={cn(
                        'flex h-8 items-center justify-center gap-1 rounded-md px-2 text-[12px] font-medium text-muted-foreground transition-colors',
                        isActive && 'bg-muted text-foreground'
                      )}
                      aria-pressed={isActive}
                      onClick={() => setCommentFilter(filter.value)}
                    >
                      <span>{filter.label}</span>
                      <span className="tabular-nums">{commentCounts[filter.value]}</span>
                    </button>
                  )
                })}
              </div>
            )}

            {comments.length === 0 ? (
              <div className="rounded-lg border border-dashed border-border/50 px-3 py-6 text-left text-[13px] text-muted-foreground">
                No comments yet.
              </div>
            ) : visibleComments.length === 0 ? (
              <div className="rounded-lg border border-dashed border-border/50 px-3 py-6 text-center text-[13px] text-muted-foreground">
                {getPRCommentAudienceEmptyLabel(commentFilter)}
              </div>
            ) : (
              <div className="flex min-w-0 flex-col gap-3">
                {visibleCommentGroups.map(renderCommentGroup)}
              </div>
            )}
          </>
        ) : null}

        {detailsLoaded && repoPath && (
          <GHCommentComposer
            className="mt-1"
            repoPath={repoPath}
            repoId={item.repoId}
            issueNumber={item.number}
            itemType={item.type}
            onCommentAdded={onCommentAdded}
          />
        )}
      </div>

      {rightPanel}
    </div>
  )
}

// Why: when the dialog opens for a Project row whose repo differs from the
// active workspace, mutations must target the row's actual repo via
// slug-addressed IPCs. Otherwise edits silently apply to the workspace's
// repo. The edit IPCs return a structured `{ ok, error }` shape; we adapt
// to a thrown rejection so the existing `useImmediateMutation` flow
// (which expects throws on failure) continues to work unchanged.
async function runIssueUpdate(args: {
  repoPath: string | null
  repoId?: string | null
  projectOrigin: GitHubItemDialogProjectOrigin | undefined
  number: number
  updates: Parameters<typeof api.gh.updateIssue>[0]['updates']
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
        ? await callRuntimeRpc<Awaited<ReturnType<typeof api.gh.updateIssueBySlug>>>(
            target,
            'github.project.updateIssueBySlug',
            updateArgs,
            { timeoutMs: 30_000 }
          )
        : await api.gh.updateIssueBySlug(updateArgs)
    if (!res.ok) {
      throw new Error(res.error.message)
    }
    return
  }
  if (!args.repoPath) {
    throw new Error('No repo context available for this edit.')
  }
  const res = await api.gh.updateIssue({
    repoPath: args.repoPath,
    repoId: args.repoId ?? undefined,
    number: args.number,
    updates: args.updates
  })
  if (!res.ok) {
    throw new Error(res.error)
  }
}

async function runWorkItemBodyUpdate(args: {
  item: GitHubWorkItem
  repoPath: string | null
  projectOrigin: GitHubItemDialogProjectOrigin | undefined
  body: string
  parsedSlug: GitHubOwnerRepo | null
}): Promise<void> {
  if (args.item.type === 'pr') {
    const targetSlug = args.projectOrigin
      ? { owner: args.projectOrigin.owner, repo: args.projectOrigin.repo }
      : args.parsedSlug
    if (!targetSlug) {
      throw new Error('No GitHub repository context available for this pull request.')
    }
    const target = getActiveRuntimeTarget(useAppStore.getState().settings)
    const updateArgs = {
      owner: targetSlug.owner,
      repo: targetSlug.repo,
      number: args.item.number,
      updates: { body: args.body }
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

  await runIssueUpdate({
    repoPath: args.repoPath,
    repoId: args.item.repoId,
    projectOrigin: args.projectOrigin,
    number: args.item.number,
    updates: { body: args.body }
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
