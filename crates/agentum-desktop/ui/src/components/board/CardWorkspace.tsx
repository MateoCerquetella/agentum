// CardWorkspace — the live drill-in for a started board card (Phase 2 + 3, #48).
//
// Three tabs over one running agent session (design 2026-06-18 "Agent workspace"):
//   • Chat — the live agent session (its tmux pane), streamed via
//     bindServerSessionTerminal (the same primitive the desktop terminal panes
//     use). The pane lives in tmux on the server, so this is a thin xterm bound
//     to its byte stream; it stays mounted across tab switches so the stream
//     (and scrollback) survive.
//   • Code — a read-only browser of the card's isolated worktree (fsListEntries
//     + fsReadFile over the embedded server's host-aware /api/fs routes).
//   • Card — the goal it's building: key, title, status, tool, worktree branch.
import { useCallback, useEffect, useRef, useState } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { ArrowLeft, FileCode, FileText, Folder, MessagesSquare, RefreshCw } from 'lucide-react'
import '@xterm/xterm/css/xterm.css'

import { cn } from '@/lib/utils'
import { buildDefaultTerminalOptions } from '@/lib/pane-manager/pane-terminal-options'
import {
  type ServerSessionTerminalBinding,
  bindServerSessionTerminal
} from '@/runtime/server-session-terminal'
import { type Session, getSession } from '@/runtime/agentum-server-client'
import { type BoardItem } from '@/runtime/board-client'
import { type FsFileEntry, fsListEntries, fsReadFile } from '@/runtime/server-fs-client'

type Tab = 'chat' | 'code' | 'card'

type CardWorkspaceProps = {
  /** The card being worked, with its bound session id. */
  item: BoardItem
  /** Return to the board. */
  onBack: () => void
}

function statusTone(status: string): string {
  switch (status) {
    case 'doing':
      return 'text-amber-500'
    case 'review':
      return 'text-sky-400'
    case 'done':
      return 'text-emerald-500'
    default:
      return 'text-muted-foreground'
  }
}

export default function CardWorkspace({ item, onBack }: CardWorkspaceProps) {
  const sessionId = item.session_id ?? ''
  const [tab, setTab] = useState<Tab>('chat')
  const [session, setSession] = useState<Session | null>(null)

  const containerRef = useRef<HTMLDivElement | null>(null)
  const bindingRef = useRef<ServerSessionTerminalBinding | null>(null)
  const fitRef = useRef<FitAddon | null>(null)

  // Fetch the session once to resolve its worktree path (the Code tab's root)
  // and surface its tool/branch in the Card tab.
  useEffect(() => {
    let alive = true
    if (sessionId) {
      void getSession(sessionId)
        .then((s) => alive && setSession(s))
        .catch(() => {})
    }
    return () => {
      alive = false
    }
  }, [sessionId])

  // Mount the live pane once and keep it alive across tab switches (we toggle
  // visibility with CSS rather than unmounting, so the WS stream persists).
  useEffect(() => {
    const container = containerRef.current
    if (!container || !sessionId) return

    const terminal = new Terminal(buildDefaultTerminalOptions())
    const fit = new FitAddon()
    fitRef.current = fit
    terminal.loadAddon(fit)
    terminal.open(container)
    try {
      fit.fit()
    } catch {
      /* pre-layout fit can throw; the observer + tab-switch re-fit recover */
    }

    const ro = new ResizeObserver(() => {
      try {
        fit.fit()
      } catch {
        /* container detached mid-teardown — ignore */
      }
    })
    ro.observe(container)

    let disposed = false
    void bindServerSessionTerminal(sessionId, terminal).then((binding) => {
      if (disposed) {
        binding.dispose()
        return
      }
      bindingRef.current = binding
    })

    return () => {
      disposed = true
      ro.disconnect()
      bindingRef.current?.dispose()
      bindingRef.current = null
      fitRef.current = null
      terminal.dispose()
    }
  }, [sessionId])

  // A hidden (display:none) xterm measures 0×0, so re-fit when Chat reappears.
  useEffect(() => {
    if (tab !== 'chat') return
    const id = setTimeout(() => {
      try {
        fitRef.current?.fit()
      } catch {
        /* ignore */
      }
    }, 0)
    return () => clearTimeout(id)
  }, [tab])

  const worktreeDir = session?.worktree_path || session?.workdir || item.workdir || null

  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-background text-foreground">
      {/* breadcrumb + tabs */}
      <div className="flex items-center gap-2 border-b border-border/60 px-3 py-2">
        <button
          type="button"
          onClick={onBack}
          className="flex items-center gap-1 rounded-md px-2 py-1 text-[12px] text-foreground/70 hover:bg-foreground/8"
        >
          <ArrowLeft className="size-3.5" />
          Board
        </button>
        <span className="text-foreground/30">/</span>
        <span className="rounded bg-foreground/10 px-1.5 py-0.5 font-mono text-[11px] text-foreground/60">
          {item.key}
        </span>
        <span className="truncate text-[13px] font-medium">{item.title}</span>

        <div className="ml-auto flex items-center gap-0.5 rounded-md border border-border/60 p-0.5">
          {(['chat', 'code', 'card'] as const).map((t) => (
            <button
              key={t}
              type="button"
              onClick={() => setTab(t)}
              className={cn(
                'rounded px-2.5 py-1 text-[12px] font-medium capitalize',
                tab === t ? 'bg-foreground/12 text-foreground' : 'text-foreground/55 hover:text-foreground'
              )}
            >
              {t}
            </button>
          ))}
        </div>
        {tab === 'chat' ? (
          <button
            type="button"
            onClick={() => bindingRef.current?.forceRedraw()}
            aria-label="Force redraw"
            title="Force redraw"
            className="flex items-center gap-1 rounded-md px-2 py-1 text-[12px] text-foreground/60 hover:bg-foreground/8"
          >
            <RefreshCw className="size-3.5" />
          </button>
        ) : null}
      </div>

      {/* Chat — always mounted, hidden when another tab is active. */}
      <div className={cn('min-h-0 flex-1', tab === 'chat' ? 'block' : 'hidden')}>
        {sessionId ? (
          <div ref={containerRef} className="h-full w-full overflow-hidden p-2" />
        ) : (
          <div className="flex h-full items-center justify-center text-[13px] text-foreground/50">
            This card has no live agent session.
          </div>
        )}
      </div>

      {tab === 'code' ? (
        <div className="min-h-0 flex-1 overflow-hidden">
          <CodeTab rootDir={worktreeDir} />
        </div>
      ) : null}

      {tab === 'card' ? (
        <div className="min-h-0 flex-1 overflow-y-auto p-5">
          <div className="mx-auto flex max-w-xl flex-col gap-3">
            <Row label="Key" value={item.key} mono />
            <Row label="Title" value={item.title} />
            <Row label="Status" value={item.status} tone={statusTone(item.status)} mono />
            <Row label="Tool" value={session?.tool ?? item.tool ?? '—'} mono />
            <Row label="Worktree branch" value={session?.worktree_branch ?? '—'} mono />
            <Row label="Worktree path" value={worktreeDir ?? '—'} mono />
            {item.body ? (
              <div className="rounded-md border border-border/60 bg-card/50 p-3">
                <div className="mb-1 text-[11px] uppercase tracking-wide text-foreground/45">
                  Goal
                </div>
                <div className="whitespace-pre-wrap text-[13px] leading-relaxed">{item.body}</div>
              </div>
            ) : null}
            <p className="text-[11px] text-foreground/40">
              Verify status follows the card's column — <span className="text-sky-400">review</span>{' '}
              is awaiting the gate, <span className="text-emerald-500">done</span> is green.
            </p>
          </div>
        </div>
      ) : null}
    </div>
  )
}

function Row({
  label,
  value,
  mono,
  tone
}: {
  label: string
  value: string
  mono?: boolean
  tone?: string
}) {
  return (
    <div className="flex items-baseline gap-3">
      <div className="w-32 shrink-0 text-[12px] text-foreground/45">{label}</div>
      <div className={cn('min-w-0 flex-1 break-words text-[13px]', mono && 'font-mono text-[12px]', tone)}>
        {value}
      </div>
    </div>
  )
}

/** Read-only worktree file browser. Lists `rootDir` and previews a clicked file. */
function CodeTab({ rootDir }: { rootDir: string | null }) {
  const [dir, setDir] = useState<string | null>(rootDir)
  const [entries, setEntries] = useState<FsFileEntry[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [file, setFile] = useState<{ path: string; content: string; isBinary: boolean } | null>(
    null
  )

  useEffect(() => {
    setDir(rootDir)
  }, [rootDir])

  const loadDir = useCallback(async (path: string) => {
    setLoading(true)
    setError(null)
    setFile(null)
    try {
      const listing = await fsListEntries(path, { hidden: false })
      setEntries(listing.entries)
      setDir(listing.path)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (dir) void loadDir(dir)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rootDir])

  const openEntry = useCallback(
    async (entry: FsFileEntry) => {
      if (entry.kind === 'dir') {
        void loadDir(entry.path)
        return
      }
      setError(null)
      try {
        // Local read — omit hostId (the server defaults to the local host).
        const c = await fsReadFile(entry.path)
        setFile({ path: entry.path, content: c.content, isBinary: c.isBinary })
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    },
    [loadDir]
  )

  if (!rootDir) {
    return (
      <div className="flex h-full items-center justify-center text-[13px] text-foreground/50">
        No worktree to browse for this card.
      </div>
    )
  }

  return (
    <div className="flex h-full min-h-0">
      <div className="flex w-72 flex-none flex-col border-r border-border/60">
        <div className="flex items-center gap-1.5 border-b border-border/60 px-3 py-2 text-[11px] text-foreground/50">
          <Folder className="size-3.5" />
          <span className="truncate font-mono">{dir ?? rootDir}</span>
          <button
            type="button"
            onClick={() => dir && void loadDir(dir)}
            className="ml-auto rounded p-0.5 hover:bg-foreground/8"
            aria-label="Refresh"
          >
            <RefreshCw className="size-3" />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto py-1">
          {loading ? (
            <div className="px-3 py-2 text-[12px] text-foreground/40">Loading…</div>
          ) : error ? (
            <div className="px-3 py-2 text-[12px] text-red-400">{error}</div>
          ) : (
            entries.map((e) => (
              <button
                key={e.path}
                type="button"
                onClick={() => void openEntry(e)}
                className="flex w-full items-center gap-1.5 px-3 py-1 text-left text-[12.5px] hover:bg-foreground/6"
              >
                {e.kind === 'dir' ? (
                  <Folder className="size-3.5 shrink-0 text-sky-400/80" />
                ) : (
                  <FileCode className="size-3.5 shrink-0 text-foreground/45" />
                )}
                <span className="truncate">{e.name}</span>
              </button>
            ))
          )}
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        {file ? (
          file.isBinary ? (
            <div className="flex h-full items-center justify-center text-[13px] text-foreground/50">
              Binary file — no preview.
            </div>
          ) : (
            <pre className="m-0 whitespace-pre p-4 font-mono text-[12px] leading-relaxed">
              {file.content}
            </pre>
          )
        ) : (
          <div className="flex h-full items-center justify-center gap-2 text-[13px] text-foreground/45">
            <FileText className="size-4" /> Select a file to preview
          </div>
        )}
      </div>
    </div>
  )
}
