// Wiki — the browse surface for AutoWiki (spec 001), a MULTI-REPO hub.
//
// agentum manages many projects, so the Wiki is a hub over ALL of them, not the
// single active workspace. Left: a Projects rail listing every repo the store
// knows (`s.repos`), each with a wiki-status dot. Select a project → its wiki in
// the main pane, reusing the existing 2-pane TOC + the editor `MarkdownPreview`
// (mermaid + `[[Title]]` links come for free).
//
// The backend is keyed by the repo's GIT IDENTITY (not the checkout path): the
// same repo cloned locally AND over SSH resolves to ONE shared wiki. So the UI
// passes `repo.id` (the server resolves host + git remote → central store). A
// local checkout and an SSH checkout of the same repo therefore show the same
// wiki — no more duplicates.
//
// Generation runs a LOCAL agent that reads the checkout, so it stays disabled for
// remote/SSH projects — but their wiki still BROWSES (a local sibling of the same
// git repo can have generated it). The agent + model are user-pickable at
// generate time (mirrors Chat); a wiki lives in the app data dir, and an opt-in
// "Save to repo" writes a committable copy back into `<repo>/.agentum/wiki`.
//
// Per-selected-repo states mirror the `GET /api/wiki` discriminator:
//   empty → the explained empty state + a "Generate wiki" button
//   running → an observable "generating…" indicator (a real session)
//   failed → the recorded error — never a half-empty success
//   ready → the TOC + the rendered page
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  AlertTriangle,
  BookText,
  Check,
  FileText,
  FolderGit2,
  Loader2,
  RefreshCw,
  Save
} from 'lucide-react'

import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import MarkdownPreview from '@/components/editor/MarkdownPreview'
import type { MarkdownDocument } from '@/shared/types'
import { AGENT_CATALOG } from '@/lib/agent-catalog'
import { CHAT_MODELS } from '@/runtime/chat-client'
import {
  exportWikiToRepo,
  generateWiki,
  getWiki,
  getWikiPage,
  type WikiIndexResponse,
  type WikiPageMeta
} from '@/runtime/wiki-client'

/** Poll cadence while a generation run is in flight. */
const RUNNING_POLL_MS = 3000

/** Picker defaults persist across restarts (same pattern as the Chat pickers).
 *  `MODEL_KEY` empty string = "the agent's own default model". */
const TOOL_KEY = 'agentum.wiki.tool'
const MODEL_KEY = 'agentum.wiki.model'

function readStored(key: string, fallback: string): string {
  try {
    return localStorage.getItem(key) ?? fallback
  } catch {
    return fallback
  }
}

/** The one-word status shown as a dot in the Projects rail. */
type RepoWikiStatus = WikiIndexResponse['state'] | 'error' | 'loading'

/** Last path segment — the human name for a repo/project. */
function repoName(path: string): string {
  return path.split('/').filter(Boolean).pop() ?? path
}

/** `<workdir>/.agentum/wiki/<slug>.md` — a stable synthetic path for
 *  MarkdownPreview's link base + scroll anchor (content itself comes from the
 *  API, so this need not be the real on-disk location). */
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

export default function WikiPage({
  pinnedRepoId
}: {
  /** Project Hub embed: lock the wiki to one project and drop the Projects
   *  rail + page title (the hub renders its own chrome). */
  pinnedRepoId?: string
} = {}): React.JSX.Element {
  const repos = useAppStore((s) => s.repos)
  const activeRepoId = useAppStore((s) => s.activeRepoId)
  const detectedAgentIds = useAppStore((s) => s.detectedAgentIds)
  const ensureDetectedAgents = useAppStore((s) => s.ensureDetectedAgents)

  // Detect installed local agents once, so the tool picker only offers what can
  // actually run (generation is local).
  useEffect(() => {
    void ensureDetectedAgents()
  }, [ensureDetectedAgents])

  // Which project's wiki we're viewing. Default to the active repo, else the
  // first; keep it valid as repos come and go.
  const [selectedRepoId, setSelectedRepoId] = useState<string | null>(pinnedRepoId ?? null)
  useEffect(() => {
    if (pinnedRepoId) {
      setSelectedRepoId(pinnedRepoId)
      return
    }
    setSelectedRepoId((cur) => {
      if (cur && repos.some((r) => r.id === cur)) return cur
      if (activeRepoId && repos.some((r) => r.id === activeRepoId)) return activeRepoId
      return repos[0]?.id ?? null
    })
  }, [repos, activeRepoId, pinnedRepoId])

  const selectedRepo = useMemo(
    () => repos.find((r) => r.id === selectedRepoId) ?? null,
    [repos, selectedRepoId]
  )
  const repoId = selectedRepo?.id ?? null
  const workdir = selectedRepo?.path ?? null
  const isRemote = selectedRepo?.connectionId != null

  // ---- agent + model pick (generation) ----
  const [tool, setTool] = useState<string>(() => readStored(TOOL_KEY, 'claude'))
  const [model, setModel] = useState<string>(() => readStored(MODEL_KEY, ''))
  useEffect(() => {
    try {
      localStorage.setItem(TOOL_KEY, tool)
    } catch {
      /* localStorage may be unavailable; picker still works in-session */
    }
  }, [tool])
  useEffect(() => {
    try {
      localStorage.setItem(MODEL_KEY, model)
    } catch {
      /* see above */
    }
  }, [model])
  // Model ids are Claude-specific; a non-Claude tool falls back to its own default.
  useEffect(() => {
    if (tool !== 'claude') setModel('')
  }, [tool])

  // Offer installed agents (until detection resolves, offer the full catalog so
  // the picker is never empty).
  const toolOptions = useMemo(() => {
    const installed = detectedAgentIds
    const list = installed
      ? AGENT_CATALOG.filter((a) => installed.includes(a.id))
      : AGENT_CATALOG
    return list.length > 0 ? list : AGENT_CATALOG.filter((a) => a.id === 'claude')
  }, [detectedAgentIds])
  const modelOptions = tool === 'claude' ? CHAT_MODELS : []

  // Per-repo status for the rail dots — a lightweight sweep so you can see which
  // projects already have a wiki, across ALL of them at a glance. Remote repos are
  // probed too now (the wiki is git-keyed, so a shared one resolves for them).
  const [repoStatuses, setRepoStatuses] = useState<Record<string, RepoWikiStatus>>({})
  const sweep = useCallback(async (): Promise<void> => {
    // Hub embed: the rail is hidden, so only the pinned project needs a probe.
    const targets = pinnedRepoId ? repos.filter((r) => r.id === pinnedRepoId) : repos
    const entries = await Promise.all(
      targets.map(async (r): Promise<[string, RepoWikiStatus]> => {
        try {
          const res = await getWiki(r.id)
          return [r.id, res.state]
        } catch {
          return [r.id, 'error']
        }
      })
    )
    setRepoStatuses(Object.fromEntries(entries))
  }, [repos, pinnedRepoId])
  useEffect(() => {
    void sweep()
  }, [sweep])

  // ---- the selected project's wiki ----
  const [index, setIndex] = useState<WikiIndexResponse | null>(null)
  const [indexError, setIndexError] = useState<string | null>(null)
  const [loadingIndex, setLoadingIndex] = useState(false)
  const [generating, setGenerating] = useState(false)
  const [saving, setSaving] = useState(false)
  const [saveMsg, setSaveMsg] = useState<string | null>(null)

  const [activeSlug, setActiveSlug] = useState<string | null>(null)
  const [pageCache, setPageCache] = useState<Record<string, string>>({})
  const [pageError, setPageError] = useState<string | null>(null)
  const [loadingPage, setLoadingPage] = useState(false)

  // A repo switch invalidates any in-flight fetch — token guards keep a late
  // response for the old repo from clobbering the new one.
  const reqToken = useRef(0)

  const refreshIndex = useCallback(
    async (id: string): Promise<void> => {
      const token = ++reqToken.current
      setLoadingIndex(true)
      setIndexError(null)
      try {
        const next = await getWiki(id)
        if (token !== reqToken.current) return
        setIndex(next)
        setRepoStatuses((prev) => ({ ...prev, [id]: next.state }))
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

  // Reload whenever the selected project changes; reset page selection + cache so
  // nothing leaks across projects.
  useEffect(() => {
    setActiveSlug(null)
    setPageCache({})
    setPageError(null)
    setSaveMsg(null)
    if (!repoId) {
      reqToken.current += 1
      setIndex(null)
      setIndexError(null)
      setLoadingIndex(false)
      return
    }
    void refreshIndex(repoId)
  }, [repoId, refreshIndex])

  // Poll while a run is in flight for the selected project.
  useEffect(() => {
    if (!repoId || index?.state !== 'running') return
    const timer = setInterval(() => void refreshIndex(repoId), RUNNING_POLL_MS)
    return () => clearInterval(timer)
  }, [repoId, index?.state, refreshIndex])

  const pages = index?.state === 'ready' ? index.pages : null

  useEffect(() => {
    if (!pages || pages.length === 0) return
    setActiveSlug((current) =>
      current && pages.some((p) => p.slug === current) ? current : pages[0].slug
    )
  }, [pages])

  useEffect(() => {
    if (!repoId || !activeSlug || index?.state !== 'ready') return
    if (pageCache[activeSlug] !== undefined) {
      setPageError(null)
      return
    }
    const token = reqToken.current
    let cancelled = false
    setLoadingPage(true)
    setPageError(null)
    void getWikiPage(repoId, activeSlug)
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
  }, [repoId, activeSlug, index?.state, pageCache])

  const handleGenerate = useCallback(async (): Promise<void> => {
    if (!repoId || generating || isRemote) return
    setGenerating(true)
    setIndexError(null)
    setSaveMsg(null)
    try {
      const { sessionId } = await generateWiki(repoId, {
        tool,
        model: model || undefined
      })
      setIndex({ state: 'running', sessionId })
      setRepoStatuses((prev) => ({ ...prev, [repoId]: 'running' }))
    } catch (err) {
      setIndexError(err instanceof Error ? err.message : String(err))
    } finally {
      setGenerating(false)
    }
  }, [repoId, generating, isRemote, tool, model])

  const handleSaveToRepo = useCallback(async (): Promise<void> => {
    if (!repoId || saving || isRemote) return
    setSaving(true)
    setSaveMsg(null)
    try {
      const res = await exportWikiToRepo(repoId)
      setSaveMsg(`Saved ${res.files} file${res.files === 1 ? '' : 's'} to the repo — commit when ready`)
    } catch (err) {
      setSaveMsg(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }, [repoId, saving, isRemote])

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

  const controls = !isRemote ? (
    <GenerationControls
      tool={tool}
      onToolChange={setTool}
      toolOptions={toolOptions}
      model={model}
      onModelChange={setModel}
      modelOptions={modelOptions}
      disabled={generating || index?.state === 'running'}
    />
  ) : null

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <header className="flex items-center justify-between gap-3 border-b border-border px-4 py-2.5">
        {/* Hub embed keeps the action strip (generate/save are per-project) but
            drops the page title — the hub header already names the project. */}
        <div className="flex items-center gap-2">
          {pinnedRepoId ? null : (
            <>
              <BookText className="size-4 text-muted-foreground" />
              <h1 className="text-sm font-semibold tracking-tight">Wiki</h1>
              {repos.length > 0 ? (
                <span className="text-xs text-muted-foreground">
                  · {repos.length} project{repos.length === 1 ? '' : 's'}
                </span>
              ) : null}
            </>
          )}
        </div>
        {index?.state === 'ready' && !isRemote ? (
          <div className="flex items-center gap-2">
            {controls}
            <Button
              variant="outline"
              size="sm"
              onClick={() => void handleSaveToRepo()}
              disabled={saving || !repoId}
              title="Write a committable copy into the repo (.agentum/wiki) so you can commit it"
            >
              {saving ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : saveMsg && !saveMsg.toLowerCase().includes('fail') ? (
                <Check className="size-3.5" />
              ) : (
                <Save className="size-3.5" />
              )}
              Save to repo
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => void handleGenerate()}
              disabled={generating || !repoId}
            >
              {generating ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : (
                <RefreshCw className="size-3.5" />
              )}
              Regenerate
            </Button>
          </div>
        ) : null}
      </header>

      {saveMsg && index?.state === 'ready' ? (
        <div className="border-b border-border bg-muted/40 px-4 py-1.5 text-xs text-muted-foreground">
          {saveMsg}
        </div>
      ) : null}

      {repos.length === 0 ? (
        <CenteredState
          icon={<FolderGit2 className="size-8 text-muted-foreground/60" />}
          title="No projects yet"
          description="Add a project from the sidebar, then generate a navigable wiki for it here."
        />
      ) : (
        <div className="flex min-h-0 flex-1">
          {pinnedRepoId ? null : (
            <RepoRail
              repos={repos}
              statuses={repoStatuses}
              selectedRepoId={selectedRepoId}
              onSelect={setSelectedRepoId}
            />
          )}
          <div className="min-w-0 flex-1">
            {renderBody({
              repoId,
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
              controls,
              onGenerate: handleGenerate,
              onOpenDocument: handleOpenDocument
            })}
          </div>
        </div>
      )}
    </div>
  )
}

// ---- the agent + model picker ----------------------------------------------

type ToolOption = { id: string; label: string }
type ModelOption = { id: string; label: string }

function GenerationControls({
  tool,
  onToolChange,
  toolOptions,
  model,
  onModelChange,
  modelOptions,
  disabled
}: {
  tool: string
  onToolChange: (v: string) => void
  toolOptions: ToolOption[]
  model: string
  onModelChange: (v: string) => void
  modelOptions: readonly ModelOption[]
  disabled?: boolean
}): React.JSX.Element {
  const selectCls =
    'h-8 rounded-md border border-border bg-card px-2 text-[12.5px] text-foreground outline-none hover:border-foreground/30 focus:border-foreground/40 disabled:opacity-50'
  return (
    <div className="flex items-center gap-1.5">
      <select
        aria-label="Wiki generation agent"
        className={selectCls}
        value={tool}
        onChange={(e) => onToolChange(e.target.value)}
        disabled={disabled}
      >
        {toolOptions.map((a) => (
          <option key={a.id} value={a.id}>
            {a.label}
          </option>
        ))}
      </select>
      {modelOptions.length > 0 ? (
        <select
          aria-label="Wiki generation model"
          className={selectCls}
          value={model}
          onChange={(e) => onModelChange(e.target.value)}
          disabled={disabled}
        >
          <option value="">Default model</option>
          {modelOptions.map((m) => (
            <option key={m.id} value={m.id}>
              {m.label}
            </option>
          ))}
        </select>
      ) : null}
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
  repoId: string | null
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
  controls: React.JSX.Element | null
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
  if (!p.repoId) {
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
              Wiki generation runs on a local agent — not available for remote/SSH projects yet.
              If you also have this repo cloned locally, generate it there and it appears here too.
            </p>
          ) : (
            <div className="flex flex-col items-center gap-2.5">
              {p.controls}
              <Button onClick={() => void p.onGenerate()} disabled={p.generating}>
                {p.generating ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : (
                  <BookText className="size-4" />
                )}
                Generate wiki
              </Button>
            </div>
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
            <div className="flex flex-col items-center gap-2.5">
              {p.controls}
              <Button variant="outline" onClick={() => void p.onGenerate()} disabled={p.generating}>
                {p.generating ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : (
                  <RefreshCw className="size-4" />
                )}
                Try again
              </Button>
            </div>
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
            <div className="flex flex-col items-center gap-2.5">
              {p.controls}
              <Button onClick={() => void p.onGenerate()} disabled={p.generating}>
                {p.generating ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : (
                  <RefreshCw className="size-4" />
                )}
                Regenerate
              </Button>
            </div>
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
            filePath={p.workdir ? pagePath(p.workdir, activeSlug) : `wiki/${activeSlug}.md`}
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
