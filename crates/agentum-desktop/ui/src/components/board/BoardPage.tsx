// Board (Kanban) — Phase 2 (#48).
//
// A real 4-column board backed by `GET /api/board`. The backend status model
// is `todo / doing / review / done` (spec 012 added `review`); this view maps
// those onto the workflow-ordered columns Backlog → Building → Review → Done
// (design 2026-06-18). Each card is a goal/ticket — goals (`lbl: 'goal'`) and
// the planner's child cards both render here; GitHub/Linear issues flow in as
// ordinary board items too. Starting/moving a card is wired in a later step.
import { useCallback, useEffect, useMemo, useState } from 'react'
import { Columns3, Loader2, Play, RefreshCw } from 'lucide-react'

import { cn } from '@/lib/utils'
import { DrillInHeader } from '@/components/nav/DrillInHeader'
import {
  type BoardItem,
  type GroupedBoard,
  listBoard,
  openBoardEventStream,
  startCard
} from '@/runtime/board-client'
import CardWorkspace from './CardWorkspace'

/**
 * The fixed column set, in workflow order. `key` is the backend `status`
 * value; `label` is the user-facing column name (design 2026-06-18). We render
 * all four columns regardless of whether the backend's `column_order` includes
 * them, so an empty `review`/`done` column still shows (and explains itself).
 */
const COLUMNS: ReadonlyArray<{ key: string; label: string; hint: string }> = [
  { key: 'todo', label: 'Backlog', hint: 'Not started yet' },
  { key: 'doing', label: 'Building', hint: 'An agent is working' },
  { key: 'review', label: 'Review', hint: 'Ready to verify' },
  { key: 'done', label: 'Done', hint: 'Shipped / verified' }
]

/** Source badge for a card — distinguishes goals from GitHub/Linear/feature cards. */
function cardSource(item: BoardItem): string {
  if (item.lbl === 'goal') return 'goal'
  return item.lbl ?? 'card'
}

function Card({
  item,
  starting,
  onStart,
  onOpen
}: {
  item: BoardItem
  starting: boolean
  onStart: (item: BoardItem) => void
  onOpen: (item: BoardItem) => void
}) {
  const isGoal = item.lbl === 'goal'
  const hasSession = Boolean(item.session_id)
  // Start is only meaningful for an unstarted feature card (a goal is a
  // container the planner fills; a card with a session is already running).
  const canStart = !isGoal && !hasSession && item.status === 'todo'

  return (
    <div
      className={cn(
        'rounded-md border border-border/60 bg-card/60 p-2.5 shadow-sm',
        hasSession && 'cursor-pointer hover:border-foreground/30'
      )}
      onClick={hasSession ? () => onOpen(item) : undefined}
      role={hasSession ? 'button' : undefined}
    >
      <div className="flex items-center gap-2">
        <span className="rounded bg-foreground/10 px-1.5 py-0.5 font-mono text-[10px] text-foreground/60">
          {item.key}
        </span>
        <span className="rounded bg-foreground/[0.06] px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-foreground/45">
          {cardSource(item)}
        </span>
        {hasSession ? (
          <span
            className="ml-auto inline-flex size-2 shrink-0 rounded-full bg-emerald-500"
            title="An agent session is bound to this card — click to watch live"
          />
        ) : null}
      </div>
      <div className="mt-1.5 text-[13px] font-medium leading-snug">{item.title}</div>
      {item.workdir ? (
        <div className="mt-1 truncate font-mono text-[10px] text-foreground/40">
          {item.workdir}
        </div>
      ) : null}
      {item.tool ? (
        <div className="mt-1 text-[10px] text-foreground/45">{item.tool}</div>
      ) : null}
      {canStart ? (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation()
            onStart(item)
          }}
          disabled={starting}
          className={cn(
            'mt-2 flex w-full items-center justify-center gap-1.5 rounded-md px-2 py-1 text-[12px] font-medium',
            starting
              ? 'cursor-not-allowed bg-foreground/10 text-foreground/40'
              : 'bg-foreground/10 text-foreground hover:bg-foreground/20'
          )}
        >
          {starting ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : (
            <Play className="size-3.5" />
          )}
          {starting ? 'Starting…' : 'Start'}
        </button>
      ) : null}
    </div>
  )
}

export default function BoardPage() {
  const [board, setBoard] = useState<GroupedBoard | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [startingId, setStartingId] = useState<number | null>(null)
  // The card whose live agent workspace is open in the drill-in.
  const [workspace, setWorkspace] = useState<BoardItem | null>(null)

  const refresh = useCallback(async () => {
    try {
      const next = await listBoard()
      setBoard(next)
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  // Start a card: spawn its agent (per-card worktree + shared launch path),
  // then drill straight into the live workspace so the user watches it work.
  const onStart = useCallback(
    async (item: BoardItem) => {
      setStartingId(item.id)
      setError(null)
      try {
        const started = await startCard(item.id)
        if (started.session_id) {
          setWorkspace(started)
        }
        await refresh()
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setStartingId(null)
      }
    },
    [refresh]
  )

  const onOpen = useCallback((item: BoardItem) => {
    if (item.session_id) setWorkspace(item)
  }, [])

  if (workspace) {
    return (
      <CardWorkspace
        item={workspace}
        onBack={() => {
          setWorkspace(null)
          void refresh()
        }}
      />
    )
  }

  // Live + polled: subscribe to the global bus so a card transitions columns
  // the instant its agent's lifecycle fires (started → Building, finished →
  // Review/Done). A slow poll remains as a backstop for the planner adding
  // cards and for any event the socket missed during a reconnect.
  useEffect(() => {
    void refresh()
    const t = setInterval(() => void refresh(), 5000)
    let stream: { close: () => void } | null = null
    let cancelled = false
    void openBoardEventStream(() => void refresh()).then((s) => {
      if (cancelled) s.close()
      else stream = s
    })
    return () => {
      cancelled = true
      clearInterval(t)
      stream?.close()
    }
  }, [refresh])

  // Bucket every board item into its column by `status`. Unknown statuses fall
  // into Backlog so a card is never silently dropped from the board.
  const byColumn = useMemo(() => {
    const buckets: Record<string, BoardItem[]> = {
      todo: [],
      doing: [],
      review: [],
      done: []
    }
    if (board) {
      for (const item of Object.values(board.columns).flat()) {
        const key = buckets[item.status] ? item.status : 'todo'
        buckets[key].push(item)
      }
    }
    return buckets
  }, [board])

  const totalCards = useMemo(
    () => Object.values(byColumn).reduce((n, items) => n + items.length, 0),
    [byColumn]
  )

  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-background text-foreground">
      <DrillInHeader
        icon={Columns3}
        title="Board"
        description="Your Kanban of agent tickets — Backlog → Building → Review → Done"
        actions={
          <button
            type="button"
            onClick={() => void refresh()}
            aria-label="Refresh"
            className="flex items-center gap-1 rounded-md px-2 py-1 text-[12px] text-foreground/60 hover:bg-foreground/8"
          >
            <RefreshCw className="size-3.5" />
            Refresh
          </button>
        }
      />

      {error ? (
        <div className="mx-4 mt-3 rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-[12px] text-red-400">
          {error}
        </div>
      ) : null}

      {loading ? (
        <div className="flex flex-1 items-center justify-center gap-2 text-[13px] text-foreground/50">
          <Loader2 className="size-4 animate-spin" /> Loading board…
        </div>
      ) : totalCards === 0 ? (
        <div className="flex flex-1 items-center justify-center p-8">
          <div className="max-w-md text-center">
            <div className="mx-auto mb-4 flex size-12 items-center justify-center rounded-xl border border-border/60 bg-card/50">
              <Columns3 className="size-6 text-muted-foreground" />
            </div>
            <h2 className="text-base font-semibold tracking-tight">No cards yet</h2>
            <p className="mt-2 text-sm text-muted-foreground">
              Describe a goal in <span className="font-medium">Chat</span> to create
              cards, or connect GitHub/Linear so issues flow in. Each card is a
              ticket you can hand to an agent — move it Backlog → Building → Review →
              Done.
            </p>
          </div>
        </div>
      ) : (
        <div className="flex flex-1 gap-3 overflow-x-auto p-4">
          {COLUMNS.map((col) => {
            const items = byColumn[col.key] ?? []
            return (
              <section
                key={col.key}
                className="flex min-w-[260px] flex-1 flex-col rounded-lg border border-border/50 bg-muted/20"
              >
                <header className="flex items-center justify-between border-b border-border/50 px-3 py-2">
                  <div className="flex items-center gap-2">
                    <span className="text-[13px] font-semibold">{col.label}</span>
                    <span className="rounded-full bg-foreground/10 px-1.5 py-0.5 text-[10px] text-foreground/55">
                      {items.length}
                    </span>
                  </div>
                </header>
                <div className="flex flex-1 flex-col gap-2 overflow-y-auto p-2">
                  {items.length === 0 ? (
                    <div className="px-1 py-3 text-center text-[11px] text-foreground/35">
                      {col.hint}
                    </div>
                  ) : (
                    items.map((item) => (
                      <Card
                        key={item.id}
                        item={item}
                        starting={startingId === item.id}
                        onStart={onStart}
                        onOpen={onOpen}
                      />
                    ))
                  )}
                </div>
              </section>
            )
          })}
        </div>
      )}
    </div>
  )
}
