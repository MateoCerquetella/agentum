import { api } from '@/tauri'
import { ChecksTab } from './pull-request-checks-tab'
import { parseOwnerRepoFromItemUrl } from '@/lib/github-item-url'
import { PRActionsPanel } from './pull-request-actions-panel'
import type { PullRequestPageProjectOrigin } from './pull-request-types'
import { GHCommentComposer } from './pull-request-comment-composer'
import { CommentReplyForm } from './pull-request-comment-reply-form'
import { MentionTextarea } from './pull-request-mention-textarea'
import { PRReviewersPanel } from './pull-request-reviewers-panel'
import { findNearestBraceBlock, getPRFileContentCacheKey, getPRFileDiffResult, getPRFileSectionKey, gitHubPRFileToBranchEntry, isPRFileViewed } from '@/lib/github-pr-detail-helpers'
import { buildMentionOptions } from './pull-request-mentions'
import { addIssueCommentForRepo, addPRReviewCommentForRepo, addPRReviewCommentReplyForRepo } from '@/lib/github-repo-operations'
import { CommentReactions, PRViewedCheckbox } from './github-item-display'
import { formatRelativeTime } from '@/lib/relative-time'
import React, { Suspense, lazy, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { editor } from 'monaco-editor'
import { ArrowDown, ArrowUp, Braces, Check, ExternalLink, LoaderCircle, MessageSquare, MessageSquarePlus, PanelLeftOpen, Pencil, UndoDot, X } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { ButtonGroup } from '@/components/ui/button-group'
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from '@/components/ui/accordion'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import CommentMarkdown from '@/components/sidebar/CommentMarkdown'
import { detectLanguage } from '@/lib/language-detect'
import { cn } from '@/lib/utils'
import { setWithLRU } from '@/lib/scroll-cache'
import { isScreenSubmitShortcut } from '@/lib/screen-submit-shortcut'
import { DiffSectionItem } from '@/components/editor/DiffSectionItem'
import type { DecoratedDiffComment } from '@/components/diff-comments/useDiffCommentDecorator'
import { CombinedDiffFileTree, createCombinedDiffSectionIndexMap, handleCombinedDiffFileTreeNavigation } from '@/components/editor/CombinedDiffFileTree'
import { getDiffSectionEstimatedHeight, isIntrinsicHeightImageDiff } from '@/components/editor/diff-section-layout'
import type { DiffSection } from '@/components/editor/diff-section-types'
import type { CombinedDiffFileTreeEntry } from '@/components/editor/combined-diff-file-tree-model'
import { filterPRCommentsByAudience, getPRCommentAudienceCounts, getPRCommentAudienceEmptyLabel, PR_COMMENT_AUDIENCE_FILTERS, type PRCommentAudienceFilter } from '@/lib/pr-comment-audience'
import { getPRCommentGroupCount, getPRCommentGroupId, getPRCommentGroupRoot, groupPRComments, isResolvedPRCommentGroup, PR_COMMENT_OPEN_AUTHOR_CLASS, PR_COMMENT_RESOLVED_AUTHOR_CLASS, PR_COMMENT_RESOLVED_CONTAINER_CLASS, type PRCommentGroup } from '@/lib/pr-comment-groups'
import { createCommentCodeContextExpansionState, resolveCommentCodeContextExpansionState, updateCommentCodeContextExpansionState, type CommentCodeContextLineUpdate } from '@/components/comment-code-context-state'
import { resolveCommentReplyTarget } from '@/components/comment-reply-target-state'
import { useAppStore } from '@/store'
import { callRuntimeRpc, getActiveRuntimeTarget } from '@/runtime/runtime-rpc-client'
import { useRepoAssignees } from '@/hooks/useIssueMetadata'
import type { GitHubOwnerRepo, GitHubPRFile, GitHubPRFileContents, GitHubWorkItem, GitHubWorkItemDetails, GitHubAssignableUser, GitBranchChangeEntry, GitDiffResult, PRCheckDetail, PRComment } from '../../../shared/types'

// Why: the GH item dialog can be opened from any work-item list surface and
// doesn't have the full owner/repo context the list's cache entry carries.
// Parsing the canonical `https://github.com/{owner}/{repo}/...` URL is the
// simplest reliable source — the URL is already present on every work item
// and survives the main-process → IPC boundary. Non-GitHub hosts return null,
// which matches the indicator's suppression rule.
const MonacoCodeExcerpt = lazy(() => import('@/components/editor/MonacoCodeExcerpt'))

const CODE_CONTEXT_EXPAND_STEP = 5

const CODE_CONTEXT_FALLBACK_LINES = 20

const CODE_CONTEXT_MAX_BLOCK_LINES = CODE_CONTEXT_FALLBACK_LINES * 2 + 1

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

type CachedPRFilesDiffViewState = {
  entrySignature: string
  sections: DiffSection[]
  sectionHeights: Record<number, number>
  loadedIndices: number[]
  scrollTop: number
  sideBySide: boolean
  fileTreeCollapsed: boolean
  activeTreeSectionKey: string | null
}

const prFilesDiffViewStateCache = new Map<string, CachedPRFilesDiffViewState>()

const prFilesDiffScrollTopCache = new Map<string, number>()

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

export function PRFilesCombinedDiffViewer({
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
  const viewStateKey = useMemo(
    () => [repoId || repoPath, prNumber].join('\0'),
    [prNumber, repoId, repoPath]
  )
  const [sections, setSections] = useState<DiffSection[]>([])
  const [sideBySide, setSideBySide] = useState(false)
  const [fileTreeCollapsed, setFileTreeCollapsed] = useState(false)
  const [sectionHeights, setSectionHeights] = useState<Record<number, number>>({})
  const [activeTreeSectionKey, setActiveTreeSectionKey] = useState<string | null>(null)
  const scrollContainerRef = useRef<HTMLDivElement>(null)
  const pendingRestoreScrollTopRef = useRef<number | null>(null)
  const loadedIndicesRef = useRef<Set<number>>(new Set())
  const loadingIndicesRef = useRef<Set<number>>(new Set())
  const sectionsRef = useRef<DiffSection[]>([])
  const generationRef = useRef(0)
  const modifiedEditorsRef = useRef<Map<number, monacoEditor.IStandaloneCodeEditor>>(new Map())
  const handleSectionSaveRef = useRef<(index: number) => Promise<void>>(async () => {})
  sectionsRef.current = sections

  useEffect(() => {
    // Why: even cached restores represent a new PR/file generation; stale async
    // diff loads from the previous view must not patch the restored sections.
    generationRef.current += 1
    const cached = prFilesDiffViewStateCache.get(viewStateKey)
    if (cached && cached.entrySignature === entrySignature) {
      const restoredSections = cached.sections
      loadedIndicesRef.current = new Set(
        cached.loadedIndices.filter((index) => !restoredSections[index]?.loading)
      )
      loadingIndicesRef.current.clear()
      setSections(restoredSections)
      setSectionHeights(cached.sectionHeights)
      setSideBySide(cached.sideBySide)
      setFileTreeCollapsed(cached.fileTreeCollapsed)
      setActiveTreeSectionKey(cached.activeTreeSectionKey)
      pendingRestoreScrollTopRef.current =
        prFilesDiffScrollTopCache.get(viewStateKey) ?? cached.scrollTop
      return
    }

    loadedIndicesRef.current.clear()
    loadingIndicesRef.current.clear()
    pendingRestoreScrollTopRef.current = prFilesDiffScrollTopCache.get(viewStateKey) ?? null
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
  }, [entries, entrySignature, viewStateKey])

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

  useEffect(() => {
    if (sections.length === 0 && entries.length > 0) {
      return
    }
    const preservedScrollTop =
      prFilesDiffScrollTopCache.get(viewStateKey) ?? scrollContainerRef.current?.scrollTop ?? 0
    setWithLRU(prFilesDiffViewStateCache, viewStateKey, {
      entrySignature,
      sections,
      sectionHeights,
      loadedIndices: Array.from(loadedIndicesRef.current).filter(
        (index) => !sections[index]?.loading
      ),
      scrollTop: preservedScrollTop,
      sideBySide,
      fileTreeCollapsed,
      activeTreeSectionKey
    })
  }, [
    activeTreeSectionKey,
    entries.length,
    entrySignature,
    fileTreeCollapsed,
    sectionHeights,
    sections,
    sideBySide,
    viewStateKey
  ])

  useLayoutEffect(() => {
    const container = scrollContainerRef.current
    if (!container) {
      return
    }

    const updateCachedScrollPosition = (): void => {
      const existing = prFilesDiffViewStateCache.get(viewStateKey)
      setWithLRU(prFilesDiffScrollTopCache, viewStateKey, container.scrollTop)
      if (!existing || existing.entrySignature !== entrySignature) {
        return
      }
      setWithLRU(prFilesDiffViewStateCache, viewStateKey, {
        ...existing,
        scrollTop: container.scrollTop
      })
    }

    container.addEventListener('scroll', updateCachedScrollPosition)
    return () => {
      updateCachedScrollPosition()
      container.removeEventListener('scroll', updateCachedScrollPosition)
    }
  }, [entrySignature, viewStateKey])

  useLayoutEffect(() => {
    const container = scrollContainerRef.current
    const targetScrollTop = pendingRestoreScrollTopRef.current
    if (!container || targetScrollTop === null) {
      return
    }

    let frameId = 0
    let attempts = 0
    const restoreScrollPosition = (): void => {
      const liveContainer = scrollContainerRef.current
      const liveTarget = pendingRestoreScrollTopRef.current
      if (!liveContainer || liveTarget === null) {
        return
      }

      const maxScrollTop = Math.max(0, liveContainer.scrollHeight - liveContainer.clientHeight)
      const nextScrollTop = Math.min(liveTarget, maxScrollTop)
      liveContainer.scrollTop = nextScrollTop
      setWithLRU(prFilesDiffScrollTopCache, viewStateKey, nextScrollTop)

      if (Math.abs(liveContainer.scrollTop - liveTarget) <= 1 || maxScrollTop >= liveTarget) {
        pendingRestoreScrollTopRef.current = null
        return
      }

      attempts += 1
      if (attempts < 30) {
        frameId = window.requestAnimationFrame(restoreScrollPosition)
      }
    }

    restoreScrollPosition()
    return () => window.cancelAnimationFrame(frameId)
  }, [sectionHeights, sections, viewStateKey])

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
      <div className="sticky top-0 z-20 flex shrink-0 items-center justify-between gap-3 border-b border-border bg-background px-3 py-1.5">
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

export function ConversationTab({
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
  participants: detailsParticipants,
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
  participants: GitHubAssignableUser[]
  localState: GitHubWorkItem['state']
  onStateChange: (state: GitHubWorkItem['state']) => void
  projectOrigin: PullRequestPageProjectOrigin | undefined
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
  const bodyTextareaRef = useRef<HTMLTextAreaElement>(null)
  const bodyTextareaFocusFrameRef = useRef<number | null>(null)
  const repoAssignees = useRepoAssignees(repoPath, item.repoId)
  const commentCounts = useMemo(() => getPRCommentAudienceCounts(comments), [comments])
  const visibleComments = useMemo(
    () => filterPRCommentsByAudience(comments, commentFilter),
    [commentFilter, comments]
  )
  const visibleCommentGroups = useMemo(() => groupPRComments(visibleComments), [visibleComments])
  const resolvedReplyingTo = resolveCommentReplyTarget(replyingTo, visibleComments)
  const mentionOptions = useMemo(
    () =>
      buildMentionOptions({
        item,
        comments,
        participants: detailsParticipants,
        assignableUsers: repoAssignees.data
      }),
    [comments, detailsParticipants, item, repoAssignees.data]
  )

  const cancelBodyTextareaFocusFrame = useCallback((): void => {
    if (bodyTextareaFocusFrameRef.current !== null) {
      cancelAnimationFrame(bodyTextareaFocusFrameRef.current)
      bodyTextareaFocusFrameRef.current = null
    }
  }, [])

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

  useEffect(() => {
    if (!bodyEditing) {
      cancelBodyTextareaFocusFrame()
      return cancelBodyTextareaFocusFrame
    }
    cancelBodyTextareaFocusFrame()
    bodyTextareaFocusFrameRef.current = requestAnimationFrame(() => {
      bodyTextareaFocusFrameRef.current = null
      bodyTextareaRef.current?.focus()
    })
    return cancelBodyTextareaFocusFrame
  }, [bodyEditing, cancelBodyTextareaFocusFrame])

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
        <aside className="overflow-hidden rounded-lg border border-border/50 bg-card shadow-xs">
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
        'min-w-0 overflow-hidden rounded-lg border border-border/40 bg-card shadow-xs',
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
            mentionOptions={mentionOptions}
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
          className="rounded-lg border border-border/40 bg-card"
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
        <div className="rounded-lg border border-border/50 bg-card shadow-xs">
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
              <MentionTextarea
                textareaRef={bodyTextareaRef}
                value={bodyDraft}
                onValueChange={setBodyDraft}
                onKeyDown={(event) => {
                  if (event.key === 'Escape') {
                    event.preventDefault()
                    setBodyDraft(body)
                    setBodyEditing(false)
                    return
                  }
                  if (isScreenSubmitShortcut(event)) {
                    event.preventDefault()
                    void handleSaveBody()
                  }
                }}
                placeholder="Description"
                rows={12}
                mentionOptions={mentionOptions}
                wrapperClassName="flex min-h-64 w-full items-stretch"
                className="scrollbar-sleek block min-h-64 w-full resize-y rounded-md border border-input bg-background px-3 py-2 font-mono text-[13px] leading-5 placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
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
            mentionOptions={mentionOptions}
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
  projectOrigin: PullRequestPageProjectOrigin | undefined
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
  projectOrigin: PullRequestPageProjectOrigin | undefined
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
