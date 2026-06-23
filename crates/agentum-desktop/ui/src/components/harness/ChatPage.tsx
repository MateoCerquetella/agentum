// Chat — the Spec→tickets FRONT DOOR (Phase 3, #48). Describe a feature in
// plain words; the planner reads the repo and decomposes it into an ordered
// backlog of BOARD CARDS (goals + child cards on `/api/board`, via the 011
// TaskSink seam — one source of truth, not a separate `.harness/` file). You
// then review the drafted cards and start them from the Board (Phase 2).
//
// Left: your goals ("chats"). Right: the conversation + the drafted cards +
// the (un-gated) composer. Backed by board-client over the embedded server.
import { type FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Columns3, Loader2, MessagesSquare, Plus, Send, Sparkles, Trash2, TriangleAlert } from 'lucide-react'

import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import { api } from '@/tauri'
import { DrillInHeader } from '@/components/nav/DrillInHeader'
import { type AgentInfo, listAgents } from '@/runtime/agentum-server-client'
import {
  type BoardItem,
  type GoalWithChildren,
  createGoal,
  deleteGoalWithChildren,
  listBoard,
  openBoardEventStream,
  selectGoalsWithChildren
} from '@/runtime/board-client'

// The planner runs as a *tool* (claude/codex/…); the agent picker choice is
// remembered across app restarts so the user doesn't re-pick every time.
const PLANNER_TOOL_KEY = 'agentum.chat.plannerTool'
function readStoredTool(): string {
  try {
    return localStorage.getItem(PLANNER_TOOL_KEY) || 'claude'
  } catch {
    return 'claude'
  }
}

// A managed account the planner can run as, for the account picker. Only Claude
// and Codex have managed accounts; other tools fall back to their live login.
type AccountOption = { id: string; email: string }
function accountProviderFor(tool: string): 'claude' | 'codex' | null {
  if (tool === 'claude') return 'claude'
  if (tool === 'codex') return 'codex'
  return null
}

// Card status → colour, matching the board's column language.
function statusColor(status: string): { dot: string; text: string } {
  switch (status) {
    case 'doing':
      return { dot: 'bg-amber-500', text: 'text-amber-500' }
    case 'review':
      return { dot: 'bg-sky-400', text: 'text-sky-400' }
    case 'done':
      return { dot: 'bg-emerald-500', text: 'text-emerald-500' }
    default:
      return { dot: 'bg-muted-foreground/40', text: 'text-muted-foreground' }
  }
}

/** Roll a goal's child statuses up into one dot for the chats list. */
function goalRollup(children: BoardItem[]): { dot: string; subtitle: string } {
  if (children.length === 0) return { dot: 'bg-muted-foreground/40', subtitle: 'planning…' }
  const done = children.filter((c) => c.status === 'done').length
  const building = children.some((c) => c.status === 'doing')
  if (done === children.length) return { dot: 'bg-emerald-500', subtitle: `${children.length} cards · done` }
  if (building) return { dot: 'bg-amber-500', subtitle: `${done}/${children.length} cards · building` }
  return { dot: 'bg-muted-foreground/40', subtitle: `${children.length} cards · drafted` }
}

export default function ChatPage() {
  const repos = useAppStore((s) => s.repos)
  const setActiveView = useAppStore((s) => s.setActiveView)

  const [goals, setGoals] = useState<GoalWithChildren[]>([])
  const [selectedId, setSelectedId] = useState<number | null>(null)
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // The goal id we just created and are waiting on the planner to fill, so the
  // thread can show a "drafting…" state until its first card lands.
  const [pendingGoalId, setPendingGoalId] = useState<number | null>(null)
  const streamRef = useRef<{ close: () => void } | null>(null)

  // Which agent runs the planner (and the cards inherit). Populated from the
  // installed-agents probe so the picker only offers tools that are on PATH.
  // The choice persists across restarts (PLANNER_TOOL_KEY).
  const [agents, setAgents] = useState<AgentInfo[]>([])
  const [selectedTool, setSelectedTool] = useState<string>(readStoredTool)
  useEffect(() => {
    try {
      localStorage.setItem(PLANNER_TOOL_KEY, selectedTool)
    } catch {
      // localStorage can be unavailable (private mode / non-browser host) — the
      // picker still works this session, it just won't be remembered.
    }
  }, [selectedTool])
  useEffect(() => {
    let alive = true
    void listAgents()
      .then((list) => {
        if (!alive) return
        setAgents(list)
        // Keep the remembered tool if it's still installed; otherwise fall back
        // (prefer claude) so we never sit on an uninstalled selection.
        const available = list.filter((a) => a.available)
        setSelectedTool((cur) => {
          if (available.some((a) => a.name === cur)) return cur
          if (available.some((a) => a.name === 'claude')) return 'claude'
          return available[0]?.name ?? cur
        })
      })
      .catch(() => {})
    return () => {
      alive = false
    }
  }, [])

  // Which managed account the planner runs as. Default = the currently active
  // account (a no-op at submit). Picking a *different* one swaps the global live
  // login before spawn — on macOS Claude reads one global Keychain, so there is
  // no per-process isolation; this is an explicit, warned, machine-wide switch.
  const [accountOptions, setAccountOptions] = useState<AccountOption[]>([])
  const [activeAccountId, setActiveAccountId] = useState<string | null>(null)
  const [selectedAccountId, setSelectedAccountId] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    const provider = accountProviderFor(selectedTool)
    if (!provider) {
      setAccountOptions([])
      setActiveAccountId(null)
      setSelectedAccountId(null)
      return
    }
    const load = provider === 'claude' ? api.claudeAccounts.list() : api.codexAccounts.list()
    void Promise.resolve(load)
      .then((st: unknown) => {
        if (!alive) return
        const s = (st ?? {}) as {
          accounts?: { id: string; email: string }[]
          activeAccountId?: string | null
          activeAccountIdsByRuntime?: { host?: string | null }
        }
        const opts = (s.accounts ?? []).map((a) => ({ id: a.id, email: a.email }))
        // Desktop is host-only — the host runtime selection is the live account.
        const active = s.activeAccountIdsByRuntime?.host ?? s.activeAccountId ?? null
        setAccountOptions(opts)
        setActiveAccountId(active)
        setSelectedAccountId(active)
      })
      .catch(() => {
        if (!alive) return
        setAccountOptions([])
        setActiveAccountId(null)
        setSelectedAccountId(null)
      })
    return () => {
      alive = false
    }
  }, [selectedTool])

  const accountSwapPending = !!selectedAccountId && selectedAccountId !== activeAccountId

  const refresh = useCallback(async () => {
    try {
      const board = await listBoard()
      const next = selectGoalsWithChildren(board)
      setGoals(next)
      setSelectedId((cur) => cur ?? (next.length ? next[0].goal.id : null))
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  // Load + live + poll: the planner creates child cards asynchronously, so we
  // both subscribe to the bus and keep a slow poll as a backstop.
  useEffect(() => {
    void refresh()
    const t = setInterval(() => void refresh(), 4000)
    let cancelled = false
    void openBoardEventStream(() => void refresh()).then((s) => {
      if (cancelled) s.close()
      else streamRef.current = s
    })
    return () => {
      cancelled = true
      clearInterval(t)
      streamRef.current?.close()
    }
  }, [refresh])

  const selected = useMemo(
    () => goals.find((g) => g.goal.id === selectedId) ?? null,
    [goals, selectedId]
  )
  const cards = selected?.children ?? []

  // Clear the "drafting…" flag once the planner's first card lands.
  useEffect(() => {
    if (pendingGoalId != null) {
      const g = goals.find((x) => x.goal.id === pendingGoalId)
      if (g && g.children.length > 0) setPendingGoalId(null)
    }
  }, [goals, pendingGoalId])

  const submit = useCallback(
    async (e?: FormEvent) => {
      e?.preventDefault()
      const text = draft.trim()
      if (!text) return
      const workdir = repos[0]?.path
      if (!workdir) {
        setError('Open a repo first — Chat drafts a backlog grounded in your project.')
        return
      }
      setBusy(true)
      setError(null)
      try {
        // Account swap (global, before spawn): the planner pane inherits the
        // live login, so make the chosen account live first. macOS has no
        // per-process credential isolation (one Keychain), so this switches the
        // active account machine-wide — which is why the picker defaults to it.
        const provider = accountProviderFor(selectedTool)
        if (provider && selectedAccountId && selectedAccountId !== activeAccountId) {
          const swap =
            provider === 'claude'
              ? api.claudeAccounts.select({ accountId: selectedAccountId, runtime: 'host', wslDistro: null })
              : api.codexAccounts.select({ accountId: selectedAccountId, runtime: 'host', wslDistro: null })
          await Promise.resolve(swap)
          setActiveAccountId(selectedAccountId)
        }
        const { goal } = await createGoal({ title: text, workdir, tool: selectedTool })
        setDraft('')
        setSelectedId(goal.id)
        setPendingGoalId(goal.id)
        await refresh()
      } catch (e2) {
        setError(e2 instanceof Error ? e2.message : String(e2))
      } finally {
        setBusy(false)
      }
    },
    [draft, repos, refresh, selectedTool, selectedAccountId, activeAccountId]
  )

  // Delete a chat (goal) and its drafted cards. Children-first so nothing is
  // orphaned; confirm because a goal with cards may have in-flight agent work.
  const removeGoal = useCallback(
    async (g: GoalWithChildren) => {
      const n = g.children.length
      const ok = window.confirm(
        n > 0
          ? `Delete "${g.goal.title}" and its ${n} card${n === 1 ? '' : 's'}? This can't be undone.`
          : `Delete "${g.goal.title}"? This can't be undone.`
      )
      if (!ok) return
      setError(null)
      try {
        await deleteGoalWithChildren(g)
        setSelectedId((cur) => (cur === g.goal.id ? null : cur))
        setPendingGoalId((cur) => (cur === g.goal.id ? null : cur))
        await refresh()
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    },
    [refresh]
  )

  const drafting = pendingGoalId === selected?.goal.id && cards.length === 0

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <DrillInHeader
        icon={MessagesSquare}
        title="Chat"
        description="Describe what you want — the planner drafts an ordered backlog of board cards"
      />

      <div className="flex min-h-0 flex-1">
        {/* ---- chats history ---- */}
        <aside className="flex w-56 flex-none flex-col border-r border-border bg-sidebar/60">
          <div className="p-3">
            <button
              type="button"
              onClick={() => {
                setSelectedId(null)
                setError(null)
              }}
              className="flex w-full items-center gap-2 rounded-md border border-border bg-card px-3 py-2 text-[13px] font-medium hover:border-foreground/30 hover:bg-accent"
            >
              <Plus className="size-3.5" /> New feature
            </button>
          </div>
          <div className="px-3.5 pb-1.5 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
            Chats
          </div>
          <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pb-3">
            {goals.length === 0 ? (
              <div className="px-3 py-2 text-[12px] text-muted-foreground">No chats yet.</div>
            ) : (
              goals.map((g) => {
                const { goal, children } = g
                const roll = goalRollup(children)
                const active = goal.id === selectedId
                return (
                  <div key={goal.id} className="group relative">
                    <button
                      type="button"
                      onClick={() => setSelectedId(goal.id)}
                      className={cn(
                        'flex w-full flex-col gap-1 rounded-md px-2.5 py-2 pr-8 text-left',
                        active ? 'bg-accent' : 'hover:bg-foreground/5'
                      )}
                    >
                      <div className="flex items-center gap-1.5">
                        <span className={cn('size-1.5 flex-none rounded-full', roll.dot)} />
                        <span className="flex-1 truncate text-[13px]">{goal.title}</span>
                      </div>
                      <span className="truncate pl-3 font-mono text-[10.5px] text-muted-foreground">
                        {roll.subtitle}
                      </span>
                    </button>
                    {/* Hover-trash: delete the chat + its drafted cards. */}
                    <button
                      type="button"
                      onClick={() => void removeGoal(g)}
                      aria-label={`Delete chat: ${goal.title}`}
                      title="Delete chat"
                      className="absolute right-1.5 top-1.5 hidden rounded p-1 text-muted-foreground hover:bg-foreground/10 hover:text-red-400 group-hover:block"
                    >
                      <Trash2 className="size-3.5" />
                    </button>
                  </div>
                )
              })
            )}
          </div>
        </aside>

        {/* ---- chat column ---- */}
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex h-11 flex-none items-center gap-2.5 border-b border-border px-5">
            <Sparkles className="size-4 text-primary" />
            <span className="text-[13.5px] font-medium">
              {selected ? selected.goal.title : 'New feature'}
            </span>
            <span className="font-mono text-[11px] text-muted-foreground">
              agentum · spec → board cards
            </span>
            {selected ? (
              <button
                type="button"
                onClick={() => setActiveView('board')}
                className="ml-auto inline-flex items-center gap-1.5 rounded-full border border-border bg-card px-2.5 py-0.5 font-mono text-[11px] hover:border-foreground/30"
              >
                <Columns3 className="size-3" /> Open in Board
              </button>
            ) : null}
          </div>

          {/* thread */}
          <div className="min-h-0 flex-1 overflow-y-auto px-5 py-6">
            <div className="mx-auto flex max-w-[720px] flex-col gap-4">
              {!selected ? (
                <div className="rounded-lg border border-dashed border-border p-8 text-center text-muted-foreground">
                  <MessagesSquare className="mx-auto mb-3 size-6 opacity-60" />
                  <div className="text-sm">Describe a feature to begin.</div>
                  <div className="mt-1 font-mono text-[11px]">
                    The planner reads the repo, drafts a spec, and decomposes it into board cards.
                  </div>
                </div>
              ) : (
                <>
                  <Message who="you" isUser text={selected.goal.title} />
                  <Message
                    who="agentum · planner"
                    text={
                      drafting
                        ? 'Reading the repo and drafting the backlog — cards will appear here as the planner decomposes the spec…'
                        : cards.length
                          ? `Drafted ${cards.length} ordered card${cards.length === 1 ? '' : 's'}. Review them below, then open the Board to start them behind the verify gate.`
                          : 'No cards drafted yet. The planner may still be working, or you can refine the description and try again.'
                    }
                  />

                  {drafting ? (
                    <div className="flex items-center gap-2 rounded-lg border border-border bg-card/60 px-4 py-3 text-[13px] text-muted-foreground">
                      <Loader2 className="size-4 animate-spin" /> Drafting cards…
                    </div>
                  ) : null}

                  {cards.length > 0 ? (
                    <div className="rounded-lg border border-border bg-card/60 p-4">
                      <div className="mb-3 flex items-center gap-2.5">
                        <span className="font-mono text-[10px] uppercase tracking-wider text-amber-500">
                          {cards.length} card{cards.length === 1 ? '' : 's'} on the board
                        </span>
                        <span className="font-mono text-[11px] text-muted-foreground">
                          /api/board
                        </span>
                      </div>
                      <div className="flex flex-col gap-1.5">
                        {cards.map((f, i) => {
                          const c = statusColor(f.status)
                          return (
                            <div
                              key={f.id}
                              className="grid grid-cols-[26px_1fr_auto] items-center gap-2.5 rounded-md border border-border bg-background px-3 py-2 text-[13px]"
                            >
                              <span className="font-mono text-[11px] text-muted-foreground">
                                {String(i + 1).padStart(2, '0')}
                              </span>
                              <span className="truncate">{f.title}</span>
                              <span
                                className={cn(
                                  'inline-flex items-center gap-1.5 font-mono text-[10px]',
                                  c.text
                                )}
                              >
                                <span className={cn('size-1.5 rounded-full', c.dot)} />
                                {f.status}
                              </span>
                            </div>
                          )
                        })}
                      </div>
                      <div className="mt-3.5 flex flex-wrap items-center gap-3">
                        <button
                          type="button"
                          onClick={() => setActiveView('board')}
                          className="inline-flex h-9 items-center gap-2 rounded-full bg-primary px-4.5 text-[13.5px] font-medium text-primary-foreground hover:opacity-85"
                        >
                          <Columns3 className="size-4" /> Review &amp; start on the Board
                        </button>
                        <span className="font-mono text-[11px] text-muted-foreground">
                          start a card → worktree + agent
                        </span>
                      </div>
                    </div>
                  ) : null}
                </>
              )}

              {error ? (
                <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-[12px] text-red-400">
                  {error}
                </div>
              ) : null}
            </div>
          </div>

          {/* composer (un-gated) */}
          <form onSubmit={submit} className="flex-none border-t border-border px-5 pb-4.5 pt-3">
            <div className="mx-auto flex max-w-[720px] items-center gap-2.5 rounded-lg border border-border bg-card px-3 py-2.5">
              <input
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                placeholder='Try "Add a CSV export to the board"…'
                className="flex-1 bg-transparent text-[14px] text-foreground placeholder:text-muted-foreground focus:outline-none"
              />
              {/* Agent picker — which agent drafts the backlog (and the cards
                  inherit). Only installed agents are selectable. */}
              <select
                value={selectedTool}
                onChange={(e) => setSelectedTool(e.target.value)}
                aria-label="Agent that drafts the backlog"
                title="Which agent runs the planner"
                className="shrink-0 rounded-md border border-border/60 bg-background px-2 py-1 text-[12px] text-foreground/80 outline-none focus:border-foreground/40"
              >
                {(agents.length
                  ? agents
                  : [{ name: 'claude', available: true } as AgentInfo]
                ).map((a) => (
                  <option key={a.name} value={a.name} disabled={!a.available}>
                    {a.name}
                    {a.available ? '' : ' (not installed)'}
                  </option>
                ))}
              </select>
              {/* Account picker — which managed account the planner runs as.
                  Default = the active account (no swap). Only shown when the
                  chosen tool has managed accounts (Claude/Codex). */}
              {accountOptions.length > 0 ? (
                <select
                  value={selectedAccountId ?? ''}
                  onChange={(e) => setSelectedAccountId(e.target.value || null)}
                  aria-label="Account the planner runs as"
                  title="Which account the planner runs as"
                  className="max-w-[10rem] shrink-0 truncate rounded-md border border-border/60 bg-background px-2 py-1 text-[12px] text-foreground/80 outline-none focus:border-foreground/40"
                >
                  {accountOptions.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.email}
                      {a.id === activeAccountId ? ' (active)' : ''}
                    </option>
                  ))}
                </select>
              ) : null}
              <button
                type="submit"
                disabled={busy || !draft.trim()}
                className="inline-flex size-8 items-center justify-center rounded-md bg-primary text-primary-foreground hover:opacity-85 disabled:opacity-40"
                aria-label="Send"
              >
                {busy ? <Loader2 className="size-4 animate-spin" /> : <Send className="size-4" />}
              </button>
            </div>
            {/* Honest about the macOS reality: picking a non-active account
                swaps the global login for every session, not just the planner. */}
            {accountSwapPending ? (
              <div className="mx-auto mt-2 flex max-w-[720px] items-center gap-1.5 font-mono text-[10.5px] text-amber-500">
                <TriangleAlert className="size-3 flex-none" />
                Switches your active {accountProviderFor(selectedTool)} account for all sessions.
              </div>
            ) : null}
          </form>
        </div>
      </div>
    </div>
  )
}

function Message({ who, text, isUser }: { who: string; text: string; isUser?: boolean }) {
  return (
    <div className={cn('flex items-start gap-3', isUser && 'flex-row-reverse')}>
      <div
        className={cn(
          'grid size-7 flex-none place-items-center rounded-full border',
          isUser ? 'border-border bg-card' : 'border-primary/40 bg-primary/10'
        )}
      >
        {isUser ? (
          <span className="font-mono text-[10px]">you</span>
        ) : (
          <Sparkles className="size-3.5 text-primary" />
        )}
      </div>
      <div className={cn('flex min-w-0 max-w-[80%] flex-col', isUser && 'items-end')}>
        <div className="mb-1.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground">
          {who}
        </div>
        <div className="rounded-lg border border-border bg-card px-4 py-3 text-[14px] leading-relaxed">
          {text}
        </div>
      </div>
    </div>
  )
}
