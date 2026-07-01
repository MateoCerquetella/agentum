// Wiki — the browse surface for AutoWiki (spec 001), now a MULTI-REPO hub.
//
// agentum manages many projects, so the Wiki is a hub over ALL of them, not the
// single active workspace. Left: a Projects rail listing every repo the store
// knows (`s.repos`), each with a wiki-status dot. Select a project → its wiki in
// the main pane (workdir = `repo.path`), reusing the existing 2-pane TOC + the
// editor `MarkdownPreview` (as-is — mermaid + `[[Title]]` links come for free).
//
// The backend is already workdir-keyed (`/api/wiki?workdir=…`), so each repo's
// wiki lives at `<repo.path>/.agentum/wiki/` — no backend change; this is the UI
// hub that was missing.
//
// Per-selected-repo states mirror the `GET /api/wiki` discriminator:
//   empty → the explained empty state + a "Generate wiki" button
//   running → an observable "generating…" indicator (a real session)
//   failed → the recorded error — never a half-empty success
//   ready → the TOC + the rendered page
// Remote/SSH repos (`connectionId != null`) are listed but generation is disabled
// (the wiki agent runs locally) — never a silent failure.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { AlertTriangle, BookText, FileText, FolderGit2, Loader2, RefreshCw } from 'lucide-react'

import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import MarkdownPreview from '@/components/editor/MarkdownPreview'
import type { MarkdownDocument } from '@/shared/types'
import {
  generateWiki,
  getWiki,
  getWikiPage,
  type WikiIndexResponse,
  type WikiPageMeta
} from '@/runtime/wiki-client'

/** Poll cadence while a generation run is in flight. */
const RUNNING_POLL_MS = 3000

/** The one-word status shown as a dot in the Projects rail. */
type RepoWikiStatus = WikiIndexResponse['state'] | 'error' | 'loading'

/** Last path segment — the human name for a repo/project. */
function repoName(path: string): string {
  return path.split('/').filter(Boolean).pop() ?? path
}

/** `<workdir>/.agentum/wiki/<slug>.md` — the real on-disk path (MarkdownPreview
 *  link base + scroll anchor). */
function pagePath(workdir: string, slug: string): string {
  return `${workdir}/.agentum/wiki/${slug}.md`
}

/** Map a wiki page to a `MarkdownDocument` so MarkdownPreview's `[[Title]]`
 *  resolver works (it keys on `name` = the page title; the slug rides in
 *  `basename` so a click maps straight back). */
function pageToDocument(workdir: string, page: WikiPageMeta): MarkdownDocument {
  return {
    filePath: pagePath(workdir, page.slug),
    relativePath: `${page.slug}.md`,
    basename: `${page.slug}.md`,
    name: page.title
  }
}

export default function WikiPage(): React.JSX.Element {
  const repos = useAppStore((s) => s.repos)
  const activeRepoId = useAppStore((s) => s.activeRepoId)

  // Which project's wiki we're viewing. Default to the active repo, else the
  // first; keep it valid as repos come and go.
  const [selectedRepoId, setSelectedRepoId] = useState<string | null>(null)
  useEffect(() => {
    setSelectedRepoId((cur) => {
      if (cur && repos.some((r) => r.id === cur)) return cur
      if (activeRepoId && repos.some((r) => r.id === activeRepoId)) return activeRepoId
      return repos[0]?.id ?? null
    })
  }, [repos, activeRepoId])

  const selectedRepo = useMemo(
    () => repos.find((r) => r.id === selectedRepoId) ?? null,
    [repos, selectedRepoId]
  )
  const workdir = selectedRepo?.path ?? null
  const isRemote = selectedRepo?.connectionId != null

  // Per-repo status for the rail dots — a lightweight sweep so you can see which
  // projects already have a wiki, across ALL of them at a glance. Local only;
  // remote workdirs aren't probed (generation is local anyway).
  const [repoStatuses, setRepoStatuses] = useState<Record<string, RepoWikiStatus>>({})
  const sweep = useCallback(async (): Promise<void> => {
    const entries = await Promise.all(
      repos.map(async (r): Promise<[string, RepoWikiStatus]> => {
        if (r.connectionId != null) return [r.id, 'error']
        try {
          const res = await getWiki(r.path)
          return [r.id, res.state]
        } catch {
          return [r.id, 'error']
        }
      })
    )
    setRepoStatuses(Object.fromEntries(entries))
  }, [repos])
  useEffect(() => {
    void sweep()
  }, [sweep])

  // ---- the selected project's wiki (workdir = selectedRepo.path) ----
  const [index, setIndex] = useState<WikiIndexResponse | null>(null)
  const [indexError, setIndexError] = useState<string | null>(null)
  const [loadingIndex, setLoadingIndex] = useState(false)
  const [generating, setGenerating] = useState(false)

  const [activeSlug, setActiveSlug] = useState<string | null>(null)
  const [pageCache, setPageCache] = useState<Record<string, string>>({})
  const [pageError, setPageError] = useState<string | null>(null)
  const [loadingPage, setLoadingPage] = useState(false)

  // A repo switch invalidates any in-flight fetch — token guards keep a late
  // response for the old repo from clobbering the new one.
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
        setRepoStatuses((prev) =>
          selectedRepoId ? { ...prev, [selectedRepoId]: next.state } : prev
        )
      } catch (err) {
        if (token !== reqToken.current) return
        setIndex(null)
        setIndexError(err instanceof Error ? err.message : String(err))
      } finally {
        if (token === reqToken.current) setLoadingIndex(false)
      }
    },
    [selectedRepoId]
  )

  // Reload whenever the selected project changes; reset page selection + cache so
  // nothing leaks across projects.
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

  // Poll while a run is in flight for the selected project.
  useEffect(() => {
    if (!workdir || index?.state !== 'running') return
    const timer = setInterval(() => void refreshIndex(workdir), RUNNING_POLL_MS)
    return () => clearInterval(timer)
  }, [workdir, index?.state, refreshIndex])

  const pages = index?.state === 'ready' ? index.pages : null

  useEffect(() => {
    if (!pages || pages.length === 0) return
    setActiveSlug((current) =>
      current && pages.some((p) => p.slug === current) ? current : pages[0].slug
    )
  }, [pages])

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
    if (!workdir || generating || isRemote) return
    setGenerating(true)
    setIndexError(null)
    try {
      const { sessionId } = await generateWiki(workdir)
      setIndex({ state: 'running', sessionId })
      if (selectedRepoId) {
        setRepoStatuses((prev) => ({ ...prev, [selectedRepoId]: 'running' }))
      }
    } catch (err) {
      setIndexError(err instanceof Error ? err.message : String(err))
    } finally {
      setGenerating(false)
    }
  }, [workdir, generating, isRemote, selectedRepoId])

  const markdownDocuments = useMemo<MarkdownDocument[]>(
    () => (workdir && pages ? pages.map((p) => pageToDocument(workdir, p)) : []),
    [workdir, pages]
  )

  const handleOpenDocument = useCallback(
    (document: MarkdownDocument) => {
      const slug = document.basename.replace(/\.md$/i, '')
      if (pages?.some((p) => p.slug === slug)) setActiveSlug(slug)
    },
    [pages]
  )

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <header className="flex items-center justify-between border-b border-border px-4 py-2.5">
        <div className="flex items-center gap-2">
          <BookText className="size-4 text-muted-foreground" />
          <h1 className="text-sm font-semibold tracking-tight">Wiki</h1>
          {repos.length > 0 ? (
            <span className="text-xs text-muted-foreground">
              · {repos.length} project{repos.length === 1 ? '' : 's'}
            </span>
          ) : null}
        </div>
        {index?.state === 'ready' && !isRemote ? (
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

      {repos.length === 0 ? (
        <CenteredState
          icon={<FolderGit2 className="size-8 text-muted-foreground/60" />}
          title="No projects yet"
          description="Add a project from the sidebar, then generate a navigable wiki for it here."
        />
      ) : (
        <div className="flex min-h-0 flex-1">
          <RepoRail
            repos={repos}
            statuses={repoStatuses}
            selectedRepoId={selectedRepoId}
            onSelect={setSelectedRepoId}
          />
          <div className="min-w-0 flex-1">
            {renderBody({
              workdir,
              isRemote,
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
      )}
    </div>
  )
}

// ---- the Projects rail ------------------------------------------------------

type RailRepo = { id: string; path: string; connectionId: string | null }

function statusDot(status: RepoWikiStatus | undefined): React.JSX.Element {
  const cls =
    status === 'ready'
      ? 'bg-emerald-500'
      : status === 'running'
        ? 'bg-amber-500 animate-pulse'
        : status === 'failed'
          ? 'bg-destructive'
          : 'bg-muted-foreground/25'
  return <span className={cn('size-1.5 shrink-0 rounded-full', cls)} aria-hidden />
}

function RepoRail({
  repos,
  statuses,
  selectedRepoId,
  onSelect
}: {
  repos: RailRepo[]
  statuses: Record<string, RepoWikiStatus>
  selectedRepoId: string | null
  onSelect: (id: string) => void
}): React.JSX.Element {
  return (
    <nav className="w-56 shrink-0 overflow-y-auto border-r border-border bg-sidebar/40 p-2">
      <div className="px-2 pb-1.5 pt-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground/70">
        Projects
      </div>
      <ul className="flex flex-col gap-0.5">
        {repos.map((r) => {
          const isActive = r.id === selectedRepoId
          return (
            <li key={r.id}>
              <button
                type="button"
                onClick={() => onSelect(r.id)}
                aria-current={isActive ? 'true' : undefined}
                className={cn(
                  'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] transition-colors',
                  isActive
                    ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                    : 'text-sidebar-foreground/70 hover:bg-sidebar-foreground/8'
                )}
              >
                {statusDot(statuses[r.id])}
                <FolderGit2 className="size-3.5 shrink-0 opacity-70" />
                <span className="truncate">{repoName(r.path)}</span>
                {r.connectionId != null ? (
                  <span className="ml-auto text-[10px] uppercase text-muted-foreground/60">ssh</span>
                ) : null}
              </button>
            </li>
          )
        })}
      </ul>
    </nav>
  )
}

// ---- the selected project's wiki body --------------------------------------

type BodyProps = {
  workdir: string | null
  isRemote: boolean
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
        icon={<Loader2 className="size-6 animate-spin text-muted-foreground" />}
        title="Loading…"
        description="Selecting a project."
      />
    )
  }

  if (!p.index && p.loadingIndex) {
    return (
      <CenteredState
        icon={<Loader2 className="size-6 animate-spin text-muted-foreground" />}
        title="Loading wiki…"
        description="Reading the wiki for this project."
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
          p.isRemote ? (
            <p className="text-xs text-muted-foreground">
              Wiki generation runs on a local agent — not yet available for remote/SSH projects.
            </p>
          ) : (
            <Button onClick={() => void p.onGenerate()} disabled={p.generating}>
              {p.generating ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                <BookText className="size-4" />
              )}
              Generate wiki
            </Button>
          )
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
          p.isRemote ? undefined : (
            <Button variant="outline" onClick={() => void p.onGenerate()} disabled={p.generating}>
              {p.generating ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                <RefreshCw className="size-4" />
              )}
              Try again
            </Button>
          )
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
          p.isRemote ? undefined : (
            <Button onClick={() => void p.onGenerate()} disabled={p.generating}>
              {p.generating ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                <RefreshCw className="size-4" />
              )}
              Regenerate
            </Button>
          )
        }
      />
    )
  }

  const activeSlug = p.activeSlug
  const content = activeSlug ? p.pageCache[activeSlug] : undefined

  return (
    <div className="flex h-full min-h-0">
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
