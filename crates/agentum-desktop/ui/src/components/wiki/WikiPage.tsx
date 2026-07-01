// Wiki — the browse surface for the AutoWiki (spec 001). A 2-pane view: a TOC of
// the generated pages on the left, the selected page's markdown rendered by the
// EXISTING editor `MarkdownPreview` (reused as-is — no fork) on the right. The
// mermaid diagram on the Architecture page and intra-wiki `[[Title]]` links both
// come for free from MarkdownPreview (its `language-mermaid` interception and the
// `markdownDocuments` + `onOpenDocument` doc-link resolver).
//
// The target workdir is derived from the active workspace (`activeWorktreeId`)
// via `splitWorktreeIdForFilesystem` — the same id→path plumbing every
// workdir-taking surface uses. With no active workspace we never call the API
// with an empty workdir; we show a "pick a workspace" state instead.
//
// States mirror the `GET /api/wiki` discriminator (spec 001 AC-2/AC-9):
//   empty   → an explained empty state + a single "Generate wiki" button
//   running → a "generating…" indicator (the run is a real, observable session)
//   failed  → the recorded error — never a half-empty success
//   ready   → the TOC + the rendered page
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { AlertTriangle, BookText, FileText, Loader2, RefreshCw } from 'lucide-react'

import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import MarkdownPreview from '@/components/editor/MarkdownPreview'
import { splitWorktreeIdForFilesystem } from '@/shared/worktree-id'
import type { MarkdownDocument } from '@/shared/types'
import {
  generateWiki,
  getWiki,
  getWikiPage,
  type WikiIndexResponse,
  type WikiPageMeta
} from '@/runtime/wiki-client'

/** Poll cadence while a generation run is in flight; the run flips the on-disk
 *  state, so re-fetching `GET /api/wiki` is how the view learns it finished. */
const RUNNING_POLL_MS = 3000

/** `<workdir>/.agentum/wiki/<slug>.md` — the real on-disk path, used as the
 *  MarkdownPreview link base + scroll-cache anchor (the page exists on disk, so
 *  any incidental stat resolves rather than erroring). */
function pagePath(workdir: string, slug: string): string {
  return `${workdir}/.agentum/wiki/${slug}.md`
}

/** Map each wiki page to a `MarkdownDocument` so MarkdownPreview's `[[Title]]`
 *  resolver works: it keys on `name`, so `name` = the page title. `basename`
 *  carries the slug (`<slug>.md`) so a resolved click maps straight back. */
function pageToDocument(workdir: string, page: WikiPageMeta): MarkdownDocument {
  return {
    filePath: pagePath(workdir, page.slug),
    relativePath: `${page.slug}.md`,
    basename: `${page.slug}.md`,
    name: page.title
  }
}

export default function WikiPage(): React.JSX.Element {
  const activeWorktreeId = useAppStore((s) => s.activeWorktreeId)
  const workdir = useMemo(
    () =>
      activeWorktreeId
        ? (splitWorktreeIdForFilesystem(activeWorktreeId)?.worktreePath ?? null)
        : null,
    [activeWorktreeId]
  )

  const [index, setIndex] = useState<WikiIndexResponse | null>(null)
  const [indexError, setIndexError] = useState<string | null>(null)
  const [loadingIndex, setLoadingIndex] = useState(false)
  const [generating, setGenerating] = useState(false)

  const [activeSlug, setActiveSlug] = useState<string | null>(null)
  const [pageCache, setPageCache] = useState<Record<string, string>>({})
  const [pageError, setPageError] = useState<string | null>(null)
  const [loadingPage, setLoadingPage] = useState(false)

  // Why: a workdir switch invalidates any in-flight fetch — token guards keep a
  // late response for the old workdir from clobbering the new one.
  const reqToken = useRef(0)

  const refreshIndex = useCallback(
    async (dir: string): Promise<void> => {
      const token = ++reqToken.current
      setLoadingIndex(true)
      setIndexError(null)
      try {
        const next = await getWiki(dir)
        if (token !== reqToken.current) return
        setIndex(next)
      } catch (err) {
        if (token !== reqToken.current) return
        setIndex(null)
        setIndexError(err instanceof Error ? err.message : String(err))
      } finally {
        if (token === reqToken.current) setLoadingIndex(false)
      }
    },
    []
  )

  // Initial load + reload whenever the active workspace changes. A new workdir
  // resets the page selection + cache so nothing leaks across workspaces.
  useEffect(() => {
    setActiveSlug(null)
    setPageCache({})
    setPageError(null)
    if (!workdir) {
      reqToken.current += 1
      setIndex(null)
      setIndexError(null)
      setLoadingIndex(false)
      return
    }
    void refreshIndex(workdir)
  }, [workdir, refreshIndex])

  // While a run is in flight, poll until it flips to ready/failed.
  useEffect(() => {
    if (!workdir || index?.state !== 'running') return
    const timer = setInterval(() => void refreshIndex(workdir), RUNNING_POLL_MS)
    return () => clearInterval(timer)
  }, [workdir, index?.state, refreshIndex])

  const pages = index?.state === 'ready' ? index.pages : null

  // Default the selection to the first page once a ready index arrives (or when
  // the current selection is no longer present after a regen).
  useEffect(() => {
    if (!pages || pages.length === 0) return
    setActiveSlug((current) =>
      current && pages.some((p) => p.slug === current) ? current : pages[0].slug
    )
  }, [pages])

  // Fetch (and cache) the active page's markdown.
  useEffect(() => {
    if (!workdir || !activeSlug || index?.state !== 'ready') return
    if (pageCache[activeSlug] !== undefined) {
      setPageError(null)
      return
    }
    const token = reqToken.current
    let cancelled = false
    setLoadingPage(true)
    setPageError(null)
    void getWikiPage(workdir, activeSlug)
      .then((res) => {
        if (cancelled || token !== reqToken.current) return
        setPageCache((prev) => ({ ...prev, [activeSlug]: res.content }))
      })
      .catch((err) => {
        if (cancelled || token !== reqToken.current) return
        setPageError(err instanceof Error ? err.message : String(err))
      })
      .finally(() => {
        if (!cancelled && token === reqToken.current) setLoadingPage(false)
      })
    return () => {
      cancelled = true
    }
  }, [workdir, activeSlug, index?.state, pageCache])

  const handleGenerate = useCallback(async (): Promise<void> => {
    if (!workdir || generating) return
    setGenerating(true)
    setIndexError(null)
    try {
      const { sessionId } = await generateWiki(workdir)
      // Reflect the run immediately so the view shows "generating…" without
      // waiting for the next poll; the poller then tracks it to completion.
      setIndex({ state: 'running', sessionId })
    } catch (err) {
      setIndexError(err instanceof Error ? err.message : String(err))
    } finally {
      setGenerating(false)
    }
  }, [workdir, generating])

  const markdownDocuments = useMemo<MarkdownDocument[]>(
    () => (workdir && pages ? pages.map((p) => pageToDocument(workdir, p)) : []),
    [workdir, pages]
  )

  // AC-7: intra-wiki nav. The resolver hands back one of our documents; the slug
  // rides in `basename` (`<slug>.md`), so map it straight back to the selection.
  const handleOpenDocument = useCallback(
    (document: MarkdownDocument) => {
      const slug = document.basename.replace(/\.md$/i, '')
      if (pages?.some((p) => p.slug === slug)) {
        setActiveSlug(slug)
      }
    },
    [pages]
  )

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <header className="flex items-center justify-between border-b border-border px-4 py-2.5">
        <div className="flex items-center gap-2">
          <BookText className="size-4 text-muted-foreground" />
          <h1 className="text-sm font-semibold tracking-tight">Wiki</h1>
        </div>
        {index?.state === 'ready' ? (
          <Button
            variant="outline"
            size="sm"
            onClick={() => void handleGenerate()}
            disabled={generating || !workdir}
          >
            {generating ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <RefreshCw className="size-3.5" />
            )}
            Regenerate
          </Button>
        ) : null}
      </header>

      <div className="min-h-0 flex-1">
        {renderBody({
          workdir,
          index,
          loadingIndex,
          indexError,
          generating,
          pages,
          activeSlug,
          setActiveSlug,
          pageCache,
          loadingPage,
          pageError,
          markdownDocuments,
          onGenerate: handleGenerate,
          onOpenDocument: handleOpenDocument
        })}
      </div>
    </div>
  )
}

type BodyProps = {
  workdir: string | null
  index: WikiIndexResponse | null
  loadingIndex: boolean
  indexError: string | null
  generating: boolean
  pages: WikiPageMeta[] | null
  activeSlug: string | null
  setActiveSlug: (slug: string) => void
  pageCache: Record<string, string>
  loadingPage: boolean
  pageError: string | null
  markdownDocuments: MarkdownDocument[]
  onGenerate: () => void | Promise<void>
  onOpenDocument: (document: MarkdownDocument) => void
}

function CenteredState({
  icon,
  title,
  description,
  action
}: {
  icon: React.JSX.Element
  title: string
  description: string
  action?: React.JSX.Element
}): React.JSX.Element {
  return (
    <div className="flex h-full items-center justify-center p-8">
      <div className="flex max-w-md flex-col items-center gap-3 text-center">
        {icon}
        <div className="text-sm font-medium text-foreground">{title}</div>
        <p className="text-sm text-muted-foreground">{description}</p>
        {action}
      </div>
    </div>
  )
}

function renderBody(p: BodyProps): React.JSX.Element {
  if (!p.workdir) {
    return (
      <CenteredState
        icon={<BookText className="size-8 text-muted-foreground/60" />}
        title="No workspace selected"
        description="Pick a workspace from the sidebar to view or generate its wiki."
      />
    )
  }

  // First load with nothing resolved yet.
  if (!p.index && p.loadingIndex) {
    return (
      <CenteredState
        icon={<Loader2 className="size-6 animate-spin text-muted-foreground" />}
        title="Loading wiki…"
        description="Reading the wiki for this workspace."
      />
    )
  }

  if (!p.index && p.indexError) {
    return (
      <CenteredState
        icon={<AlertTriangle className="size-8 text-destructive" />}
        title="Couldn't load the wiki"
        description={p.indexError}
      />
    )
  }

  const state = p.index?.state

  if (state === 'empty' || !p.index) {
    return (
      <CenteredState
        icon={<BookText className="size-8 text-muted-foreground/60" />}
        title="No wiki yet"
        description="Generate a navigable wiki for this repo — an overview, an architecture page with a module diagram, and one page per module. An agent reads the repo and writes it; the run is observable like any other session."
        action={
          <Button onClick={() => void p.onGenerate()} disabled={p.generating}>
            {p.generating ? <Loader2 className="size-4 animate-spin" /> : <BookText className="size-4" />}
            Generate wiki
          </Button>
        }
      />
    )
  }

  if (state === 'running') {
    const sessionId = p.index?.state === 'running' ? p.index.sessionId : null
    return (
      <CenteredState
        icon={<Loader2 className="size-6 animate-spin text-muted-foreground" />}
        title="Generating wiki…"
        description="An agent is reading the repo and writing the wiki pages. This can take a few minutes; the page refreshes itself when it's done."
        action={
          sessionId ? (
            <code className="rounded bg-muted px-2 py-1 text-xs text-muted-foreground">
              session {sessionId.slice(0, 8)}
            </code>
          ) : undefined
        }
      />
    )
  }

  if (state === 'failed') {
    const error = p.index?.state === 'failed' ? p.index.error : 'wiki generation failed'
    return (
      <CenteredState
        icon={<AlertTriangle className="size-8 text-destructive" />}
        title="Wiki generation failed"
        description={error}
        action={
          <Button variant="outline" onClick={() => void p.onGenerate()} disabled={p.generating}>
            {p.generating ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}
            Try again
          </Button>
        }
      />
    )
  }

  // state === 'ready'
  const pages = p.pages ?? []
  if (pages.length === 0) {
    return (
      <CenteredState
        icon={<BookText className="size-8 text-muted-foreground/60" />}
        title="The wiki is empty"
        description="The last run produced no pages. Regenerate to rebuild it."
        action={
          <Button onClick={() => void p.onGenerate()} disabled={p.generating}>
            {p.generating ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}
            Regenerate
          </Button>
        }
      />
    )
  }

  const activeSlug = p.activeSlug
  const content = activeSlug ? p.pageCache[activeSlug] : undefined

  return (
    <div className="flex h-full min-h-0">
      {/* Left: table of contents (index.json page order). */}
      <nav className="w-60 shrink-0 overflow-y-auto border-r border-border bg-sidebar/40 p-2">
        <ul className="flex flex-col gap-0.5">
          {pages.map((page) => {
            const isActive = page.slug === activeSlug
            return (
              <li key={page.slug}>
                <button
                  type="button"
                  onClick={() => p.setActiveSlug(page.slug)}
                  aria-current={isActive ? 'page' : undefined}
                  className={cn(
                    'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] transition-colors',
                    isActive
                      ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                      : 'text-sidebar-foreground/70 hover:bg-sidebar-foreground/8'
                  )}
                >
                  <FileText className="size-3.5 shrink-0 opacity-70" />
                  <span className="truncate">{page.title}</span>
                </button>
              </li>
            )
          })}
        </ul>
      </nav>

      {/* Right: the selected page, rendered by the existing MarkdownPreview. */}
      <div className="min-w-0 flex-1">
        {activeSlug && content !== undefined ? (
          <MarkdownPreview
            key={activeSlug}
            content={content}
            filePath={pagePath(p.workdir, activeSlug)}
            scrollCacheKey={`wiki:${activeSlug}`}
            markdownDocuments={p.markdownDocuments}
            onOpenDocument={p.onOpenDocument}
          />
        ) : p.pageError ? (
          <CenteredState
            icon={<AlertTriangle className="size-8 text-destructive" />}
            title="Couldn't load this page"
            description={p.pageError}
          />
        ) : (
          <div className="flex h-full items-center justify-center">
            <Loader2 className="size-6 animate-spin text-muted-foreground" />
          </div>
        )}
      </div>
    </div>
  )
}
